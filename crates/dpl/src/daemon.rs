//! CLI half of the control protocol: connect to `dpld`'s unix socket, send
//! one [`Request`], read one [`Response`]. Blocking (the CLI has no runtime).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use anyhow::{bail, Context};
use dpl_core::ipc::{Envelope, Request, Response, MAX_FRAME};
use dpl_core::PROTOCOL_VERSION;

/// Send `request` to the daemon and return its decoded response. Surfaces a
/// friendly error if the daemon isn't running.
pub fn call(request: Request, home_override: Option<&str>) -> anyhow::Result<Response> {
    let socket = dpl_core::paths::daemon_socket(home_override)?;
    let mut stream = UnixStream::connect(&socket).map_err(|e| {
        anyhow::anyhow!(
            "could not reach dpld at {} ({e}). Is the daemon running? Start it with `dpld`.",
            socket.display()
        )
    })?;

    write_frame(&mut stream, &Envelope::new(request))?;
    let env: Envelope<Response> = read_frame(&mut stream)?;
    if env.protocol != PROTOCOL_VERSION {
        bail!(
            "protocol mismatch: client v{PROTOCOL_VERSION}, daemon v{}. Rebuild the mismatched binary.",
            env.protocol
        );
    }
    Ok(env.payload)
}

fn write_frame<T: serde::Serialize>(stream: &mut UnixStream, value: &T) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(value).context("encoding request")?;
    let len = u32::try_from(bytes.len()).context("request too large")?;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(&bytes)?;
    stream.flush()?;
    Ok(())
}

fn read_frame<T: serde::de::DeserializeOwned>(stream: &mut UnixStream) -> anyhow::Result<T> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).context("reading response length")?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME {
        bail!("response frame too large: {len} bytes");
    }
    let mut buf = vec![0u8; len as usize];
    stream.read_exact(&mut buf).context("reading response body")?;
    serde_json::from_slice(&buf).context("decoding response")
}
