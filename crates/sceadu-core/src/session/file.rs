//! Content-addressed file transfer over a dedicated libp2p stream.
//!
//! The wire format is deliberately small and resumable:
//!   frame = u32 big-endian length + payload
//!   first frame: JSON header
//!   reply:       JSON { offset }
//!   chunks:      raw bytes, each followed by JSON { offset }
//!   end:         0-length frame, followed by JSON { done: true }
//!
//! The receiver keeps `<root>/<sha256>.partial` until the whole file has been
//! received and verified, then renames it to `<root>/<sha256>`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::{AsyncReadExt, AsyncWriteExt};
use libp2p::swarm::Stream;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agent::{FileRef, MAX_FILE_BYTES};
use crate::Error;

pub const FILE_PROTOCOL: &str = "/sceadu/file/1.0.0";
pub const CHUNK_SIZE: usize = 48 * 1024;

#[derive(Clone)]
pub struct FileStore {
    root: Arc<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Header {
    name: String,
    media_type: String,
    size: u64,
    sha256: String,
    chunk_size: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct Ack {
    offset: u64,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    error: Option<String>,
}

impl FileStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, Error> {
        let root = root.into();
        std::fs::create_dir_all(&root)
            .map_err(|error| Error::Unsupported(format!("file store: {error}")))?;
        Ok(Self {
            root: Arc::new(root),
        })
    }

    pub fn path(&self, sha256: &str) -> PathBuf {
        self.root.join(sha256)
    }

    fn partial_path(&self, sha256: &str) -> PathBuf {
        self.root.join(format!("{sha256}.partial"))
    }
}

/// Receive one inbound stream into the store.
pub async fn receive(store: FileStore, mut stream: Stream) -> Result<FileRef, Error> {
    let header: Header = read_json(&mut stream).await?;
    if header.size == 0 || header.size > MAX_FILE_BYTES {
        let _ = write_json(
            &mut stream,
            &Ack {
                offset: 0,
                done: false,
                error: Some("file too large".to_owned()),
            },
        )
        .await;
        return Err(Error::Unsupported(
            "file size is outside the supported range".to_owned(),
        ));
    }
    if header.sha256.len() != 64 {
        return Err(Error::Unsupported("invalid file hash".to_owned()));
    }
    let final_path = store.path(&header.sha256);
    let partial_path = store.partial_path(&header.sha256);
    let mut offset = std::fs::metadata(&partial_path)
        .map(|meta| meta.len())
        .unwrap_or(0)
        .min(header.size);
    if final_path.exists() {
        write_json(
            &mut stream,
            &Ack {
                offset: header.size,
                done: false,
                error: None,
            },
        )
        .await?;
        return Ok(FileRef {
            name: header.name,
            media_type: header.media_type,
            sha256: header.sha256,
            size: header.size,
        });
    }
    write_json(
        &mut stream,
        &Ack {
            offset,
            done: false,
            error: None,
        },
    )
    .await?;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&partial_path)
        .map_err(|error| Error::Unsupported(format!("open partial: {error}")))?;
    use std::io::{Seek, SeekFrom, Write};
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| Error::Unsupported(format!("seek partial: {error}")))?;

    loop {
        let chunk = read_frame(&mut stream).await?;
        if chunk.is_empty() {
            break;
        }
        if offset + chunk.len() as u64 > header.size {
            let _ = write_json(
                &mut stream,
                &Ack {
                    offset,
                    done: false,
                    error: Some("file exceeds declared size".to_owned()),
                },
            )
            .await;
            return Err(Error::Unsupported("file exceeds declared size".to_owned()));
        }
        file.write_all(&chunk)
            .map_err(|error| Error::Unsupported(format!("write file: {error}")))?;
        offset += chunk.len() as u64;
        write_json(
            &mut stream,
            &Ack {
                offset,
                done: false,
                error: None,
            },
        )
        .await?;
    }
    file.flush()
        .map_err(|error| Error::Unsupported(format!("flush file: {error}")))?;
    drop(file);

    let actual = sha256_file(&partial_path)?;
    if actual != header.sha256 {
        let _ = write_json(
            &mut stream,
            &Ack {
                offset,
                done: false,
                error: Some("sha256 mismatch".to_owned()),
            },
        )
        .await;
        return Err(Error::Unsupported("file hash mismatch".to_owned()));
    }
    std::fs::rename(&partial_path, &final_path)
        .map_err(|error| Error::Unsupported(format!("finalize file: {error}")))?;
    write_json(
        &mut stream,
        &Ack {
            offset,
            done: true,
            error: None,
        },
    )
    .await?;
    Ok(FileRef {
        name: header.name,
        media_type: header.media_type,
        sha256: header.sha256,
        size: header.size,
    })
}

/// Send one local file over a newly opened stream.
pub async fn send(mut stream: Stream, path: &Path) -> Result<FileRef, Error> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| Error::Unsupported(format!("file metadata: {error}")))?;
    if metadata.len() == 0 || metadata.len() > MAX_FILE_BYTES {
        return Err(Error::Unsupported(
            "file size is outside the supported range".to_owned(),
        ));
    }
    let sha256 = sha256_file(path)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_owned());
    let media_type = mime_guess(&name);
    let header = Header {
        name: name.clone(),
        media_type: media_type.clone(),
        size: metadata.len(),
        sha256: sha256.clone(),
        chunk_size: CHUNK_SIZE,
    };
    write_json(&mut stream, &header).await?;
    let ack: Ack = read_json(&mut stream).await?;
    if let Some(error) = ack.error {
        return Err(Error::Unsupported(error));
    }
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)
        .map_err(|error| Error::Unsupported(format!("open file: {error}")))?;
    let mut offset = ack.offset.min(metadata.len());
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| Error::Unsupported(format!("seek file: {error}")))?;
    let mut buffer = vec![0u8; CHUNK_SIZE];
    while offset < metadata.len() {
        let want = (metadata.len() - offset).min(CHUNK_SIZE as u64) as usize;
        file.read_exact(&mut buffer[..want])
            .map_err(|error| Error::Unsupported(format!("read file: {error}")))?;
        write_frame(&mut stream, &buffer[..want]).await?;
        let ack: Ack = read_json(&mut stream).await?;
        if let Some(error) = ack.error {
            return Err(Error::Unsupported(error));
        }
        offset = ack.offset;
    }
    write_frame(&mut stream, &[]).await?;
    let ack: Ack = read_json(&mut stream).await?;
    if ack.error.is_some() || !ack.done {
        return Err(Error::Unsupported(
            ack.error
                .unwrap_or_else(|| "receiver did not confirm the file".to_owned()),
        ));
    }
    Ok(FileRef {
        name,
        media_type,
        sha256,
        size: metadata.len(),
    })
}

pub fn sha256_file(path: &Path) -> Result<String, Error> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)
        .map_err(|error| Error::Unsupported(format!("open file: {error}")))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| Error::Unsupported(format!("read file: {error}")))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(data_encoding::HEXLOWER.encode(&hasher.finalize()))
}

fn mime_guess(name: &str) -> String {
    match Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "mp4" => "video/mp4",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
    .to_owned()
}

async fn read_frame(stream: &mut Stream) -> Result<Vec<u8>, Error> {
    let mut length = [0u8; 4];
    stream
        .read_exact(&mut length)
        .await
        .map_err(|error| Error::Unsupported(format!("read frame length: {error}")))?;
    let length = u32::from_be_bytes(length) as usize;
    if length > CHUNK_SIZE + 4096 {
        return Err(Error::Unsupported("file frame too large".to_owned()));
    }
    let mut payload = vec![0u8; length];
    stream
        .read_exact(&mut payload)
        .await
        .map_err(|error| Error::Unsupported(format!("read frame: {error}")))?;
    Ok(payload)
}

async fn write_frame(stream: &mut Stream, payload: &[u8]) -> Result<(), Error> {
    let length = (payload.len() as u32).to_be_bytes();
    stream
        .write_all(&length)
        .await
        .map_err(|error| Error::Unsupported(format!("write frame length: {error}")))?;
    stream
        .write_all(payload)
        .await
        .map_err(|error| Error::Unsupported(format!("write frame: {error}")))?;
    stream
        .flush()
        .await
        .map_err(|error| Error::Unsupported(format!("flush frame: {error}")))?;
    Ok(())
}

async fn read_json<T: for<'de> Deserialize<'de>>(stream: &mut Stream) -> Result<T, Error> {
    let payload = read_frame(stream).await?;
    serde_json::from_slice(&payload)
        .map_err(|error| Error::Unsupported(format!("decode file header: {error}")))
}

async fn write_json<T: Serialize>(stream: &mut Stream, value: &T) -> Result<(), Error> {
    let payload = serde_json::to_vec(value)
        .map_err(|error| Error::Unsupported(format!("encode file header: {error}")))?;
    write_frame(stream, &payload).await
}
