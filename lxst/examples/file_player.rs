use std::env;
use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use lxst::FilePlayer;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let path = args
        .next()
        .unwrap_or_else(|| "speech_stereo.opus".to_string());
    let looping = args.any(|arg| arg == "--loop");

    let mut player = FilePlayer::new(path, looping)?;
    player.start()?;

    let stop_rx = if looping {
        let (stop_tx, stop_rx) = mpsc::channel();
        thread::spawn(move || {
            let mut line = String::new();
            let _ = io::stdin().read_line(&mut line);
            let _ = stop_tx.send(());
        });
        Some(stop_rx)
    } else {
        None
    };

    while player.is_playing()
        && stop_rx
            .as_ref()
            .is_none_or(|stop_rx| stop_rx.try_recv().is_err())
    {
        player.process_next()?;
        thread::sleep(Duration::from_millis(10));
    }
    player.stop()?;
    println!("Playback finished");

    Ok(())
}
