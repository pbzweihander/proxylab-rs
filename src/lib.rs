#![feature(async_await, await_macro, futures_api, pin, try_blocks)]

extern crate futures;
extern crate regex;
extern crate tokio;
#[macro_use]
extern crate lazy_static;

pub mod cache;

use futures::{
    future::ready,
    {compat::*, prelude::*},
};
use regex::Regex;
use std::io::BufRead;
use tokio::{
    io,
    prelude::{AsyncRead, AsyncWrite},
};

pub enum HttpError {
    IsDirectory(String),
    Forbidden(String),
    NotFound(String),
    NotImplemented(String),
    Error(String),
}

pub enum FileType {
    Html,
    Jpg,
    Png,
    Gif,
    PlainText,
}

pub struct Uri {
    pub host: String,
    pub port: u32,
    pub path: String,
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
        let path = caps.get(2).map(|s| ".".to_string() + s.as_str());

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
        "Content-type: text/html\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );

    let (writer, _) = await!(io::write_all(writer, line).compat())?;
    let (writer, _) = await!(io::write_all(writer, header).compat())?;
    let _ = await!(io::write_all(writer, body).compat())?;

    println!(
        "client error:\n{} {}\n{}: {}\n",
        info.0, info.1, info.2, info.3
    );

    Ok(())
}
