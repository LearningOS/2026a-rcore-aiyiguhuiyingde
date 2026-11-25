use std::fs::{read_dir, File};
use std::io::{Result, Write};
fn main() {
    println!("cargo:rerun-if-changed={}", TARGET_PATH);
    insert_app_data().unwrap();
}
static TARGET_PATH: &str = "../user/build/bin/";
fn insert_app_data() -> Result<()> {
    let mut f = File::create("src/link_app.S").unwrap();
    let mut apps: Vec<_> = read_dir("../user/build/bin/").unwrap().into_iter().map(|d| {
        let mut n = d.unwrap().file_name().into_string().unwrap();
        n.drain(n.find('.').unwrap()..n.len());
        n
    }).collect();
    apps.sort();
    writeln!(f, "\n    .align 3\n    .section .data\n    .global _num_app\n_num_app:\n    .quad {}", apps.len())?;
    for i in 0..apps.len() { writeln!(f, "    .quad app_{}_start", i)?; }
    writeln!(f, "    .quad app_{}_end", apps.len().saturating_sub(1))?;
    writeln!(f, "\n    .global _app_names\n_app_names:")?;
    for app in apps.iter() { writeln!(f, "    .string \"{}\"", app)?; }
    for (idx, app) in apps.iter().enumerate() {
        writeln!(f, "\n    .section .data\n    .global app_{0}_start\n    .global app_{0}_end\n    .align 3\napp_{0}_start:\n    .incbin \"{2}{1}.bin\"\napp_{0}_end:", idx, app, TARGET_PATH)?;
    }
    Ok(())
}
