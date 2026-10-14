use super::{http::Request, security_headers};
use std::{
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use yoke_core::store::Store;
pub(super) fn start(request: Request, database: PathBuf, connections: Arc<AtomicUsize>) {
    if connections.fetch_add(1, Ordering::AcqRel) >= 4 {
        connections.fetch_sub(1, Ordering::AcqRel);
        let _ = super::respond(request, 503, "text/plain", "Too many event streams");
        return;
    }
    std::thread::spawn(move || {
        struct Slot(Arc<AtomicUsize>);
        impl Drop for Slot {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::AcqRel);
            }
        }
        let _slot = Slot(connections);
        let Ok(store) = Store::open(database) else {
            let _ = super::respond(request, 503, "text/plain", "Event stream unavailable");
            return;
        };
        let Ok(mut revision) = store.revision() else {
            let _ = super::respond(request, 503, "text/plain", "Event stream unavailable");
            return;
        };
        let Ok(mut stream) = request.into_stream(200, security_headers("text/event-stream")) else {
            return;
        };
        if stream
            .write_all(b"event: ready\ndata: connected\n\n")
            .is_err()
        {
            return;
        }
        let started = Instant::now();
        let mut heartbeat = Instant::now();
        while started.elapsed() < Duration::from_secs(55) {
            std::thread::sleep(Duration::from_millis(500));
            let Ok(next) = store.revision() else {
                return;
            };
            let data = if next != revision {
                revision = next;
                Some(b"event: change\ndata: updated\n\n".as_slice())
            // A short heartbeat notices closed tabs quickly and frees their stream slot.
            } else if heartbeat.elapsed() >= Duration::from_secs(2) {
                Some(b": heartbeat\n\n".as_slice())
            } else {
                None
            };
            if let Some(data) = data {
                if stream.write_all(data).is_err() {
                    return;
                }
                heartbeat = Instant::now();
            }
        }
    });
}
