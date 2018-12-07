#![feature(async_await, await_macro, futures_api, pin, try_blocks)]

extern crate futures;
extern crate regex;
extern crate tokio;
#[macro_use]
extern crate lazy_static;

pub mod cache;

use futures::{
    future::ready,
    stream::iter,
    {compat::*, prelude::*},
};
use regex::Regex;
use std::{io::BufRead, iter::once};
use tokio::{
    io,
    prelude::{AsyncRead, AsyncWrite},
};

#[derive(Debug)]
pub enum HttpError {
    IsDirectory(String),
    Forbidden(String),
    NotFound(String),
    NotImplemented(String),
    Error(String),
}

#[derive(Debug)]
pub enum FileType {
    Html,
    Jpg,
    Png,
    Gif,
    PlainText,
}

#[derive(Debug)]
pub struct Uri {
    pub host: String,
    pub port: u16,
    pub path: String,
}

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub uri: Uri,
    pub version: String,
    pub headers: Vec<String>,
}

#[derive(Debug)]
pub struct Response {
    pub version: String,
    pub status: u16,
    pub reason: String,
    pub headers: Vec<String>,
    pub content: Vec<u8>,
}

pub fn get_filetype(filename: &str) -> FileType {
    if filename.ends_with(".html") {
        FileType::Html
    } else if filename.ends_with(".jpg") {
        FileType::Jpg
    } else if filename.ends_with(".png") {
        FileType::Png
    } else if filename.ends_with(".gif") {
        FileType::Gif
    } else {
        FileType::PlainText
    }
}

pub async fn print_requesthdrs(reader: impl AsyncRead + BufRead) {
    let lines = io::lines(reader);

    await!(lines
        .compat()
        .filter_map(|l| ready(l.ok()))
        .take_while(|l| ready(!l.is_empty()))
        .for_each(|l| ready(println!("{}", l))));

    println!();
}

pub fn parse_uri(uri: &str, default_host: &str) -> Option<Uri> {
    lazy_static! {
        static ref REGEX: Regex = Regex::new(r"^(?:http://)?(.*?)(/.*?)\s*$").unwrap();
    }

    REGEX.captures(uri).and_then(|caps| {
        let host = caps
            .get(1)
            .map(|s| s.as_str())
            .unwrap_or_else(|| default_host);
        let mut split = host.split(':');

        let host = split.next().unwrap_or_default().to_string();
        let port = split
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| 80);
        let path = caps.get(2).map(|s| s.as_str().to_string());

        path.map(|p| Uri {
            host: host,
            port: port,
            path: p,
        })
    })
}

pub async fn client_error(writer: impl AsyncWrite, e: HttpError) -> Result<(), io::Error> {
    let info: (u16, String, String, String) = match e {
        HttpError::Error(e) => (400, "Error".to_string(), "Error occured".to_string(), e),
        HttpError::Forbidden(e) => (
            403,
            "Forbidden".to_string(),
            "The requested file is forbidden".to_string(),
            e,
        ),
        HttpError::IsDirectory(e) => (
            403,
            "Forbidden".to_string(),
            "The requested file is a directory".to_string(),
            e,
        ),
        HttpError::NotFound(e) => (
            404,
            "NotFound".to_string(),
            "The requested file is not found".to_string(),
            e,
        ),
        HttpError::NotImplemented(e) => (
            501,
            "NotImplemented".to_string(),
            "The requested method is not implemented".to_string(),
            e,
        ),
    };

    let body = format!(
        "<html><head><title>Mini Error</title></head><body bgcolor=ffffff>\r\n\
         <b>{}: {}</b>\r\n\
         <p>{}: {}\r\n\
         <hr><em>Mini Web server</em></body></html>\r\n",
        info.0, info.1, info.2, info.3,
    );

    let line = format!("HTTP/1.0 {} {}\r\n", info.0, info.1);

    let header = format!(
        "Content-Type: text/html\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );

    let (writer, _) = await!(io::write_all(writer, line).compat())?;
    let (writer, _) = await!(io::write_all(writer, header).compat())?;
    let _ = await!(io::write_all(writer, body).compat())?;

    println!(
        "client error: {} {}\n{}: {}\n",
        info.0, info.1, info.2, info.3
    );

    Ok(())
}

async fn read_headers(
    reader: impl AsyncRead + BufRead,
) -> Result<(impl AsyncRead + BufRead, Vec<String>), HttpError> {
    let mut headers = vec![];
    let mut reader = reader;

    loop {
        let (r, header) = await!(io::read_until(reader, b'\n', vec![]).compat())
            .map_err(|e| HttpError::Error(format!("header reading failed: {:?}", e)))?;
        reader = r;
        let header = String::from_utf8_lossy(&header);
        let header = header.trim();
        if header.is_empty() {
            return Ok((reader, headers));
        }
        headers.push(header.to_string());
    }
}

pub async fn read_request(reader: impl AsyncRead + BufRead) -> Result<Request, HttpError> {
    let (reader, buf) = await!(io::read_until(reader, b'\n', vec![]).compat())
        .map_err(|e| HttpError::Error(format!("read failed: {:?}", e)))?;

    let line = String::from_utf8(buf)
        .map_err(|e| HttpError::Error(format!("decode request failed: {:?}", e)))?;
    let mut iter = line.split_whitespace();

    let method = iter.next();
    let uri = iter.next();
    let version = iter.next();

    let (method, uri, version) = method
        .and_then(|m| {
            uri.and_then(|u| version.map(|v| (m.to_string(), u.to_string(), v.to_string())))
        })
        .ok_or_else(|| HttpError::Error("request line parsing failed".to_string()))?;

    let (_, headers) = await!(read_headers(reader))?;
    let mut request_host = String::new();
    for header in headers.iter() {
        if header.starts_with("Host:") {
            request_host = header
                .split_whitespace()
                .nth(1)
                .map(|s| s.to_string())
                .ok_or_else(|| HttpError::Error("parse host failed".to_string()))?;
        }
    }
    let uri = parse_uri(&uri, &request_host)
        .ok_or_else(|| HttpError::Error("parse uri failed".to_string()))?;

    Ok(Request {
        method,
        uri,
        version,
        headers,
    })
}

pub async fn read_response(reader: impl AsyncRead + BufRead) -> Result<Response, HttpError> {
    let (reader, buf) = await!(io::read_until(reader, b'\n', vec![]).compat())
        .map_err(|e| HttpError::Error(format!("read failed: {:?}", e)))?;

    let line = String::from_utf8(buf)
        .map_err(|e| HttpError::Error(format!("decode request failed: {:?}", e)))?;
    let mut iter = line.split_whitespace();

    let version = iter.next().map(|s| s.to_string());
    let status = iter.next().and_then(|s| s.parse().ok());
    let reason = iter.map(|s| s.to_string()).collect::<Vec<_>>().join(" ");

    let (version, status, reason) = version
        .and_then(|v| status.map(|s| (v, s, reason)))
        .ok_or_else(|| HttpError::Error("status line parsing failed".to_string()))?;

    let (reader, headers) = await!(read_headers(reader))?;

    let mut content_length = 0usize;
    let mut is_chunked = false;
    for header in headers.iter() {
        if header.starts_with("Content-Length:") {
            is_chunked = false;
            content_length = header
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| HttpError::Error("parse content length failed".to_string()))?;
        } else if header.starts_with("Transfer-Encoding:") && header.contains("chunked") {
            is_chunked = true;
            content_length = 0;
        }
    }

    let content = if is_chunked {
        let mut content = Vec::new();
        let mut reader = reader;
        loop {
            let (r, chunk_size) = await!(io::read_until(reader, b'\n', vec![]).compat())
                .map_err(|e| HttpError::Error(format!("chunk size reading failed: {:?}", e)))?;
            let chunk_size: usize = String::from_utf8_lossy(&chunk_size)
                .parse()
                .map_err(|e| HttpError::Error(format!("chunk size parsing failed: {:?}", e)))?;
            if chunk_size == 0 {
                break;
            }
            let (r, mut chunk) = await!(io::read_exact(r, vec![0; chunk_size]).compat())
                .map_err(|e| HttpError::Error(format!("chunk reading failed: {:?}", e)))?;
            content.append(&mut chunk);
            reader = r;
        }
        content
    } else {
        await!(io::read_exact(reader, vec![0; content_length]).compat())
            .map(|(_, c)| c)
            .map_err(|e| HttpError::Error(format!("content reading failed: {:?}", e)))?
    };

    Ok(Response {
        version,
        status,
        reason,
        headers,
        content,
    })
}

pub async fn request(writer: impl AsyncWrite + Send, req: Request) -> Result<(), HttpError> {
    let url = if req.uri.port == 80 {
        format!("http://{}{}", req.uri.host, req.uri.path)
    } else {
        format!("http://{}:{}{}", req.uri.host, req.uri.port, req.uri.path)
    };
    let req_line = format!("{} {} {}\r\n\r\n", req.method, url, req.version);

    let fut = io::write_all(writer, req_line).compat();

    let fut = fut
        .and_then(|(writer, _)| {
            iter(req.headers.into_iter().chain(once(String::new())))
                .map(|hdr| hdr + "\r\n")
                .fold(
                    Ok(writer),
                    |acc, hdr| -> std::pin::Pin<Box<dyn Future<Output = _> + Send>> {
                        if let Ok(writer) = acc {
                            io::write_all(writer, hdr)
                                .compat()
                                .map(|r| r.map(|(writer, _)| writer))
                                .boxed()
                        } else {
                            ready(acc).boxed()
                        }
                    },
                )
        })
        .map_err(|e| HttpError::Error(format!("sending failed: {:?}", e)));
    await!(fut)?;

    Ok(())
}

pub async fn response(writer: impl AsyncWrite, resp: Response) -> Result<(), io::Error> {
    let line = format!("{} {} {}\r\n", resp.version, resp.status, resp.reason);

    let (writer, _) = await!(io::write_all(writer, line).compat())?;
    let mut writer = writer;
    for header in resp.headers {
        let w = writer;
        let (w, _) = await!(io::write_all(w, header + "\r\n").compat())?;
        writer = w;
    }
    let (writer, _) = await!(io::write_all(writer, b"\r\n").compat())?;
    let _ = await!(io::write_all(writer, resp.content).compat())?;

    Ok(())
}
