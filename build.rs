fn main() {
    slint_build::compile("ui/app.slint").expect("failed to compile Slint UI");
    println!("cargo:rerun-if-changed=ui/app.slint");
    println!("cargo:rerun-if-changed=ui/theme.slint");
    println!("cargo:rerun-if-changed=ui/widgets.slint");
    println!("cargo:rerun-if-changed=ui/assets/icon.png");
    println!("cargo:rerun-if-changed=ui/assets/hero.png");
}
