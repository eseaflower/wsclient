
fn generate_gl_bindings() {
    // let dest = std::path::PathBuf::from(&std::env::var("OUT_DIR").unwrap());

    let dest = std::path::PathBuf::from(&"bindings");
    let mut file = std::fs::File::create(&dest.join("test_gl_bindings.rs")).unwrap();
    gl_generator::Registry::new(
        gl_generator::Api::Gl,
        (4, 5),
        gl_generator::Profile::Core,
        gl_generator::Fallbacks::All,
        [],
    )
    .write_bindings(gl_generator::StructGenerator, &mut file)
    .unwrap();
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    generate_gl_bindings();
}