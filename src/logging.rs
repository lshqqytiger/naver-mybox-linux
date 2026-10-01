use std::{io::Write, os::unix::net::UnixDatagram};

use crate::Result;

struct Syslog(UnixDatagram);

struct SyslogWriter<'socket> {
    socket: &'socket UnixDatagram,
    bytes: Vec<u8>,
}

impl<'socket> tracing_subscriber::fmt::MakeWriter<'socket> for Syslog {
    type Writer = SyslogWriter<'socket>;

    fn make_writer(&'socket self) -> Self::Writer {
        SyslogWriter {
            socket: &self.0,
            bytes: format!("<14>myboxfs[{}]: ", std::process::id()).into_bytes(),
        }
    }
}

impl Write for SyslogWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for SyslogWriter<'_> {
    fn drop(&mut self) {
        let _ = self.socket.send(&self.bytes);
    }
}

pub fn init(daemon: bool) -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(if cfg!(debug_assertions) {
            "debug"
        } else {
            "info"
        })
    });
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact();
    if daemon {
        let socket = UnixDatagram::unbound()?;
        socket
            .connect("/dev/log")
            .map_err(|_| "cannot open /dev/log; use foreground with supervised stderr logging")?;
        subscriber
            .with_ansi(false)
            .with_writer(Syslog(socket))
            .try_init()?;
    } else {
        subscriber.with_writer(std::io::stderr).try_init()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_post_startup_diagnostic_in_daemon_destination() {
        let (sender, receiver) = UnixDatagram::pair().unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(Syslog(sender))
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("MYBOX flush failed after startup");
        });
        let mut bytes = [0; 4096];
        let length = receiver.recv(&mut bytes).unwrap();
        let message = std::str::from_utf8(&bytes[..length]).unwrap();
        assert!(message.starts_with("<14>myboxfs["));
        assert!(message.contains("MYBOX flush failed after startup"));
    }
}
