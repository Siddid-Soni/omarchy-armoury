fn main() {
    let text = std::fs::read_to_string(armoury_root::SUPPORT_FILE).unwrap();
    let fix = armoury_root::fix_for_board("G533ZW").unwrap();
    match armoury_root::patch_support(&text, fix) {
        Ok(Some(new)) => println!("would patch: {} -> {} bytes", text.len(), new.len()),
        Ok(None) => println!("already patched"),
        Err(e) => println!("error: {e}"),
    }
}
