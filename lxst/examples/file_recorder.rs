use std::env;
use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use lxst::FileRecorder;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .unwrap_or_else(|| "recording.opus".to_string());
    let mut recorder = FileRecorder::new(&path)?;
    let (stop_tx, stop_rx) = mpsc::channel();

    thread::spawn(move || {
        let mut line = String::new();
        let _ = io::stdin().read_line(&mut line);
        let _ = stop_tx.send(());
    });

    println!("Recording to {path}; press Enter to stop");
    recorder.start();
    while stop_rx.try_recv().is_err() {
        recorder.process_next()?;
        thread::sleep(Duration::from_millis(10));
    }
    recorder.stop()?;
    println!("Recording saved to {path}");

    Ok(())
}
