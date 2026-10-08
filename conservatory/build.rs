fn main() {
    // CWD is the crate dir; the assets live at the workspace root.
    glib_build_tools::compile_resources(
        &["../data/icons"],
        "../data/icons/conservatory.gresource.xml",
        "conservatory.gresource",
    );
}
