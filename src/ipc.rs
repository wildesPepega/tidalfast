//! Single-instance support: a second `tidalfast <link>` hands its argument to
//! the running instance instead of opening another window.

use std::sync::mpsc::Receiver;

#[cfg(unix)]
mod imp {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::mpsc::{self, Receiver};

    fn socket_path() -> PathBuf {
        dirs::runtime_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("tidalfast.sock")
    }

    /// Send `message` to a running instance. True if one took it.
    pub fn forward(message: &str) -> bool {
        match UnixStream::connect(socket_path()) {
            Ok(mut stream) => stream.write_all(format!("{message}\n").as_bytes()).is_ok(),
            Err(_) => false,
        }
    }

    pub fn listen(ctx: eframe::egui::Context) -> Receiver<String> {
        let (tx, rx) = mpsc::channel();
        let path = socket_path();
        let _ = std::fs::remove_file(&path);
        match UnixListener::bind(&path) {
            Ok(listener) => {
                std::thread::spawn(move || {
                    for stream in listener.incoming().flatten() {
                        for line in BufReader::new(stream).lines().map_while(Result::ok) {
                            let _ = tx.send(line);
                            ctx.request_repaint();
                        }
                    }
                });
            }
            Err(e) => log::warn!("single-instance socket unavailable: {e}"),
        }
        rx
    }
}

#[cfg(not(unix))]
mod imp {
    use std::sync::mpsc::{self, Receiver};

    pub fn forward(_message: &str) -> bool {
        false
    }

    pub fn listen(_ctx: eframe::egui::Context) -> Receiver<String> {
        mpsc::channel().1
    }
}

pub fn forward(message: &str) -> bool {
    imp::forward(message)
}

pub fn listen(ctx: eframe::egui::Context) -> Receiver<String> {
    imp::listen(ctx)
}
