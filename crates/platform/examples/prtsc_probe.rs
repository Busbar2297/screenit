//! Manual probe: prints a line for every Print Screen press seen by the platform listener.
//! `cargo run -p screenit-platform --example prtsc_probe -- [dpi] [hotkeys]`

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "dpi") {
        screenit_platform::init_dpi_awareness();
    }
    let ok = screenit_platform::keyhook::spawn_print_screen_hook(|| println!("print screen"));
    println!("listener installed: {ok}");
    if args.iter().any(|a| a == "hotkeys") {
        let (tx, _rx) = std::sync::mpsc::channel();
        let keys: Vec<_> = screenit_capture::parse_hotkey("Ctrl+Shift+F")
            .into_iter()
            .collect();
        let failed = screenit_platform::hotkeys::spawn(keys, tx, || {});
        println!("hotkeys failed: {failed:?}");
    }
    std::thread::sleep(std::time::Duration::from_secs(12));
}
