use std::fs;
use std::path::Path;
use regex::Regex;

fn generate_web_sys_objects(js_folder: &str, output_file: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut rust_code = String::new();
    rust_code.push_str("// Generated web-sys objects\n");
    rust_code.push_str("use wasm_bindgen::prelude::*;\n");
    rust_code.push_str("use web_sys::*;\n\n");

    let js_regex = Regex::new(r"const\s+([a-zA-Z0-9_]+)\s*=\s*([a-zA-Z0-9_.]+);")?;

    for entry in fs::read_dir(js_folder)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("js") {
            let file_content = fs::read_to_string(&path)?;

            for capture in js_regex.captures_iter(&file_content) {
                let object_name = capture.get(1).map(|m| m.as_str()).unwrap_or_default();
                let js_object = capture.get(2).map(|m| m.as_str()).unwrap_or_default();

                if !object_name.is_empty() && !js_object.is_empty() {
                    let rust_object = js_object.replace(".", "::");

                    rust_code.push_str(&format!(
                        "#[wasm_bindgen(js_name = {})]\n",
                        js_object
                    ));
                    rust_code.push_str(&format!(
                        "extern \"C\" {{\n    static mut {}: {};\n}}\n\n",
                        object_name, rust_object
                    ));
                }
            }
        }
    }

    fs::write(output_file, rust_code)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let js_folder = "./static/js"; // Replace with your JavaScript folder
    let output_file = "./src/web_sys_objects.rs";

    generate_web_sys_objects(js_folder, output_file)?;

    println!("Web-sys objects generated in {}", output_file);
    Ok(())
}
