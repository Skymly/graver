fn main() {
    // MSVC warns that COM entry points should be PRIVATE when it also writes
    // the import library. The exports themselves are intentional.
    println!("cargo:rustc-link-arg=/IGNORE:4104");
}
