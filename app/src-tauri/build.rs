fn main() {
    // The webview pages are built by Vite into ../dist (`npm run build` in
    // app/). Tauri embeds that folder at compile time, so a plain
    // `cargo build` / `cargo test` on a fresh clone would fail without it.
    // Leave a placeholder page instead and say so.
    let dist = std::path::Path::new("../dist");
    if !dist.join("index.html").exists() {
        std::fs::create_dir_all(dist).expect("creating app/dist");
        std::fs::write(
            dist.join("index.html"),
            "<!doctype html><meta charset=utf-8><title>Deskhand</title>\
             <p>The Deskhand UI is not built. Run <code>npm ci &amp;&amp; npm run build</code> in <code>app/</code>, then build again.</p>",
        )
        .expect("writing the placeholder page");
        println!("cargo:warning=app/dist was missing: wrote a placeholder page. Run `npm ci && npm run build` in app/ for the real UI.");
    }
    println!("cargo:rerun-if-changed=../dist");
    tauri_build::build()
}
