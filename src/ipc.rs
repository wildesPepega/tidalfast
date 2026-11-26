//! Single-instance support: a second `oxytide <link>` hands its argument to
//! the running instance instead of opening another window.

use std::sync::mpsc::Receiver;

#[cfg(unix)]
mod imp {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::mpsc::{self, Receiver};

    use std::io::Read;
    use std::time::Duration;

    /// Longest message accepted (a link, or "focus").
    const MAX_LINE: u64 = 4096;

    fn socket_path() -> PathBuf {
        // a private folder: in the shared temp folder another user could take the name first
        crate::fsx::runtime_dir().join("oxytide.sock")
    }

    /// Send `message` to a running instance. True if one took it.
    pub fn forward(message: &str) -> bool {
        match UnixStream::connect(socket_path()) {
            Ok(mut stream) => {
                let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                stream.write_all(format!("{message}\n").as_bytes()).is_ok()
            }
            Err(_) => false,
        }
    }

    pub fn listen(ctx: eframe::egui::Context) -> Receiver<String> {
        let (tx, rx) = mpsc::channel();
        let path = socket_path();
        let _ = std::fs::remove_file(&path);
        match UnixListener::bind(&path) {
            Ok(listener) => {
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
                }
                std::thread::spawn(move || {
                    for stream in listener.incoming().flatten() {
                        // a client that connects and says nothing must not block the next one
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        let reader = BufReader::new(stream.take(MAX_LINE * 4));
                        for line in reader.lines().map_while(Result::ok).take(4) {
                            if line.len() as u64 > MAX_LINE {
                                continue;
                            }
                            #[cfg(target_os = "linux")]
                            crate::kwin::wake_if_hidden();
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
