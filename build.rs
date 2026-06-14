fn main() {
    println!("cargo:rustc-env=SLINT_BACKEND=winit-skia");
    println!("cargo:rustc-env=SLINT_ENABLE_EXPERIMENTAL_FEATURES=1");
    println!("cargo:rustc-env=SLINT_SKIA_PARTIAL_RENDERING=1");

    slint_build::compile("ui/app-window.slint").expect("Slint build failed");
}
