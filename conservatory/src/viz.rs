//! The spectrum audio tap (Phase 12d). libmpv exposes no PCM tap, so the
//! visualizer captures the PipeWire **default sink monitor** (what is playing,
//! never altering it) on its own thread, runs the core FFT analyzer, and
//! publishes log-spaced band levels into a shared buffer the GTK widget reads on
//! its frame clock.
//!
//! All PipeWire objects are `Rc`-based and thread-affine, so the entire capture
//! lives inside the spawned thread; only the `Arc<Mutex<Vec<f32>>>` band buffer
//! and a `pipewire::channel` terminate signal cross the boundary.
//!
//! Caveat: a sink-monitor tap sees *all* system audio, not only Conservatory's.
//! When something else plays, the bars react too. That is inherent to capturing
//! the output device without a private loopback.

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use conservatory_core::player::spectrum::{SpectrumAnalyzer, FFT_SIZE};

use pipewire as pw;
use pw::spa;
use pw::spa::pod::Pod;

/// The number of spectrum bars. Log-spaced from ~32 Hz to Nyquist.
pub const N_BANDS: usize = 28;

/// A running capture: the shared band buffer the widget polls, plus the handle to
/// stop the capture thread. Dropping or calling [`SpectrumTap::stop`] tears the
/// PipeWire stream down.
pub struct SpectrumTap {
    bands: Arc<Mutex<Vec<f32>>>,
    terminate: pw::channel::Sender<()>,
    handle: Option<JoinHandle<()>>,
}

impl SpectrumTap {
    /// Start capturing the default sink monitor. Returns immediately; the capture
    /// runs on a dedicated thread until [`stop`](Self::stop) (or drop).
    pub fn start() -> Self {
        let bands = Arc::new(Mutex::new(vec![0.0_f32; N_BANDS]));
        let (tx, rx) = pw::channel::channel::<()>();
        let bands_thread = bands.clone();
        let handle = std::thread::Builder::new()
            .name("spectrum-capture".into())
            .spawn(move || {
                if let Err(e) = run_capture(bands_thread, rx) {
                    tracing::warn!("spectrum capture stopped: {e}");
                }
            })
            .ok();
        Self {
            bands,
            terminate: tx,
            handle,
        }
    }

    /// The latest raw band levels (0..=1), cloned. Smoothing happens widget-side
    /// at frame rate, so this is the un-smoothed snapshot.
    pub fn bands(&self) -> Vec<f32> {
        self.bands
            .lock()
            .map(|b| b.clone())
            .unwrap_or_else(|_| vec![0.0; N_BANDS])
    }

    /// Signal the capture thread to quit and join it.
    pub fn stop(mut self) {
        let _ = self.terminate.send(());
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for SpectrumTap {
    fn drop(&mut self) {
        let _ = self.terminate.send(());
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Per-stream state carried through the PipeWire listener callbacks.
struct Capture {
    format: spa::param::audio::AudioInfoRaw,
    analyzer: Option<SpectrumAnalyzer>,
    window: Vec<f32>,
    bands: Arc<Mutex<Vec<f32>>>,
}

fn run_capture(
    bands: Arc<Mutex<Vec<f32>>>,
    rx: pw::channel::Receiver<()>,
) -> Result<(), pw::Error> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;

    // Quit the loop when the GTK side signals (widget unmapped / app closing).
    let _recv = rx.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });

    // `STREAM_CAPTURE_SINK=true` makes this an input that records the default
    // sink's monitor (what is playing), following the default device.
    let props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
        *pw::keys::STREAM_CAPTURE_SINK => "true",
        *pw::keys::NODE_NAME => "Conservatory Visualizer",
    };

    let stream = pw::stream::StreamBox::new(&core, "conservatory-visualizer", props)?;

    let data = Capture {
        format: Default::default(),
        analyzer: None,
        window: Vec::with_capacity(FFT_SIZE * 2),
        bands,
    };

    let _listener = stream
        .add_local_listener_with_user_data(data)
        .param_changed(|_, data, id, param| {
            let Some(param) = param else { return };
            if id != pw::spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = pw::spa::param::format_utils::parse_format(param)
            else {
                return;
            };
            if media_type != pw::spa::param::format::MediaType::Audio
                || media_subtype != pw::spa::param::format::MediaSubtype::Raw
            {
                return;
            }
            if data.format.parse(param).is_ok() {
                let rate = data.format.rate().max(1);
                data.analyzer = Some(SpectrumAnalyzer::new(N_BANDS, rate));
            }
        })
        .process(|stream, data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            let Some(d0) = datas.first_mut() else {
                return;
            };
            let n_channels = data.format.channels().max(1) as usize;
            let size = d0.chunk().size() as usize;
            let Some(bytes) = d0.data() else {
                return;
            };
            let n_floats = (size / 4).min(bytes.len() / 4);
            let n_frames = n_floats / n_channels;

            // Downmix interleaved f32 frames to mono and append to the rolling
            // window, keeping only the most recent FFT_SIZE samples.
            for f in 0..n_frames {
                let mut acc = 0.0_f32;
                for c in 0..n_channels {
                    let idx = (f * n_channels + c) * 4;
                    acc += f32::from_le_bytes([
                        bytes[idx],
                        bytes[idx + 1],
                        bytes[idx + 2],
                        bytes[idx + 3],
                    ]);
                }
                data.window.push(acc / n_channels as f32);
            }
            if data.window.len() > FFT_SIZE {
                let drop = data.window.len() - FFT_SIZE;
                data.window.drain(0..drop);
            }

            if let Some(analyzer) = data.analyzer.as_mut() {
                let levels = analyzer.analyze(&data.window);
                if let Ok(mut shared) = data.bands.lock() {
                    shared.copy_from_slice(levels);
                }
            }
        })
        .register()?;

    // Request F32 audio; rate / channels are left for the server to fill in (we
    // read them back in `param_changed`).
    let mut audio_info = spa::param::audio::AudioInfoRaw::new();
    audio_info.set_format(spa::param::audio::AudioFormat::F32LE);
    let obj = pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: pw::spa::param::ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .map_err(|_| pw::Error::CreationFailed)?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&values).ok_or(pw::Error::CreationFailed)?];

    stream.connect(
        spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;

    mainloop.run();
    Ok(())
}
