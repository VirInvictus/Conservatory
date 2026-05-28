//! Conservatory GTK4 binary. The GUI arrives at Phase 3 (spec §17); until
//! then this is a placeholder so the workspace builds without the GTK
//! toolchain present.

fn main() {
    println!("Conservatory {}", conservatory_core::VERSION);
    println!("GUI not built yet (Phase 3, see spec.md §17). Use conservatory-cli.");
}
