//! The voice chat server's socket: one thread reads datagrams and answers them through the
//! [`Core`], and sends the keep-alives. It never calls into the game; what the game must hear
//! (a connection that checked out or timed out) is left in the core's events for the next tick.

use crate::core::Core;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The running server: the thread and how to stop it.
pub struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub address: SocketAddr,
}

fn lock(core: &Mutex<Core>) -> MutexGuard<'_, Core> {
    core.lock().unwrap_or_else(|e| e.into_inner())
}

impl Running {
    /// Binds the socket and starts the thread. The core's port becomes the one bound, which is
    /// what the config's 0 asks the system for.
    pub fn start(core: Arc<Mutex<Core>>) -> io::Result<Self> {
        let (bind, port) = {
            let core = lock(&core);
            let bind = if core.config.bind_address.trim().is_empty() {
                "0.0.0.0".to_owned()
            } else {
                core.config.bind_address.trim().to_owned()
            };
            (bind, core.config.port)
        };
        let socket = UdpSocket::bind((bind.as_str(), port))?;
        socket.set_read_timeout(Some(Duration::from_millis(10)))?;
        let address = socket.local_addr()?;
        lock(&core).port = address.port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("VoiceChatServerThread".into())
            .spawn(move || serve(&core, &socket, &flag))?;
        Ok(Self {
            stop,
            thread: Some(thread),
            address,
        })
    }

    /// Stops the thread, closing the socket.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve(core: &Mutex<Core>, socket: &UdpSocket, stop: &AtomicBool) {
    let mut buffer = vec![0u8; 4096];
    let mut last_keep_alive = Instant::now();
    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        let interval = Duration::from_millis(lock(core).config.keep_alive.max(1000) as u64);
        if now.duration_since(last_keep_alive) >= interval {
            last_keep_alive = now;
            let packets = lock(core).keep_alives(now);
            for packet in packets {
                let _ = socket.send_to(&packet.bytes, packet.to);
            }
        }
        match socket.recv_from(&mut buffer) {
            Ok((len, from)) => {
                let packets = lock(core).handle(Instant::now(), from, &buffer[..len]);
                for packet in packets {
                    let _ = socket.send_to(&packet.bytes, packet.to);
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            // A datagram that bounced back from an address that is gone, or another error that
            // does not end the socket.
            Err(_) => {}
        }
    }
}
