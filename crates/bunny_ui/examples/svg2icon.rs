//! The offline half of the SVG door: read icon files, print Rust
//! const source to paste into your app.
//!
//! ```sh
//! cargo run -p bunny-ui --features svg --example svg2icon -- icons/*.svg
//! ```
//!
//! Each file becomes one `Symbol` const named after the file stem
//! (`chevron-down.svg` → `CHEVRON_DOWN`, name `"chevron.down"`),
//! normalized onto the house 24 grid. Notes about collapsed colors
//! print as comments — a clean file converts silently.

#[cfg(feature = "svg")]
fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: svg2icon <file.svg>…");
        std::process::exit(2);
    }
    println!("use bunny_ui::icon::{{Draw, Glyph, Paint, Rule, Symbol}};");
    println!("use bunny_ui::icon::Verb::{{Close, Cubic, Line, Move, Quad}};");
    println!();
    let mut failed = false;
    for file in files {
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("{file}: {error}");
                failed = true;
                continue;
            }
        };
        let stem = std::path::Path::new(&file)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("icon");
        let const_name: String = stem
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
            .collect();
        let name = stem.replace(['-', '_'], ".");
        match bunny_ui::icon::parse::parse(&text) {
            Ok(parsed) => {
                print!("{}", bunny_ui::icon::parse::to_rust_const(&const_name, &name, &parsed));
                println!();
            }
            Err(error) => {
                eprintln!("{file}: {error}");
                failed = true;
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
}

#[cfg(not(feature = "svg"))]
fn main() {
    eprintln!("the svg door is closed — run with: --features svg");
    std::process::exit(2);
}