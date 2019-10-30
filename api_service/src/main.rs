extern crate mime;
extern crate image;

use std::cell::Cell;
use std::fs;
use std::io::Write;
use std::str::FromStr;
use rand::prelude::*;
use actix_multipart::{Field, Multipart, MultipartError};
use actix_web::{error, middleware, web, App, Error, HttpResponse, HttpServer, guard};
use actix_http::RequestHead;
use futures::future::{err, Either};
use futures::{Future, Stream, stream};
use serde::{Deserialize};
use std::time::{SystemTime, UNIX_EPOCH};
use bytes::{Buf, IntoBuf};
use reqwest::r#async::{Client, Chunk};
use mime::Mime;
use crate::image::GenericImageView;

const PARALLEL_REQUESTS: usize = 20;
const MAX_CONTENT_LENGTH: usize = 10;
const IMAGE_FOLDER: &'static str = "imgs";


pub struct AppState {
    pub counter: Cell<usize>,
}

#[derive(Deserialize)]
pub struct UploadImageJson {
    pub urls: Vec<String>
}

fn generate_filename() -> String {
    let now = SystemTime::now();
    let timestamp = now.duration_since(UNIX_EPOCH).expect("Wrong system time");
    let rnd = thread_rng().gen::<u32>();

    format!("{}_{}", rnd, timestamp.as_millis())
}

fn get_file_ext_from_path(filepath: &str) -> &str {
    filepath
        .split(".")
        .collect::<Vec<&str>>()
        .last()
        .cloned()
        .unwrap()
}

fn handle_image_multipart(field: Field) -> impl Future<Item = String, Error = Error> {
    if field.content_type().type_() != mime::IMAGE{
        return Either::A(err(error::ErrorInternalServerError("File is not an image")))
    }

    let content_disposition = field.content_disposition().unwrap();
    let uploaded_filename = content_disposition.get_filename().unwrap();
    let file_extension = get_file_ext_from_path(uploaded_filename);

    let filename = generate_filename();
    let filename_ext = format!("{}.{}", filename, file_extension);
    
    let file = match fs::File::create(&format!("./{}/{}", IMAGE_FOLDER, &filename_ext)) {
        Ok(file) => file,
        Err(e) => return Either::A(err(error::ErrorInternalServerError(e))),
    };
    Either::B(
        field
            //saving file
            .fold((file, Vec::new()), move |(mut file, mut acc_bytes), bytes| {
                //running blocking fs method in thread pool
                web::block(move || {
                    file.write_all(bytes.as_ref()).map_err(|e| {
                        println!("file.write_all failed: {:?}", e);
                        MultipartError::Payload(error::PayloadError::Io(e))
                    })?;
                    acc_bytes.extend_from_slice(bytes.as_ref());
                    Ok((file, acc_bytes))
                })
                .map_err(|e: error::BlockingError<MultipartError>| {
                    match e {
                        error::BlockingError::Error(e) => e,
                        error::BlockingError::Canceled => MultipartError::Incomplete,
                    }
                })
            })
            //generating preview
            .map(|(_, data)| {
                //running blocking fs method in thread pool
                web::block(move || {
                    generate_preview(&data, &filename).unwrap();
                    Ok(filename_ext)
                })
                .map_err(|e: error::BlockingError<MultipartError>| {
                    match e {
                        error::BlockingError::Error(e) => e,
                        error::BlockingError::Canceled => MultipartError::Incomplete,
                    }
                })
            })
            .flatten()
            .map(|filename| filename)
            .map_err(|e| {
                println!("handle_file_multipart failed, {:?}", e);
                error::ErrorInternalServerError(e)
            }),
    )
}

fn generate_preview(bytes: &[u8], name: &str) -> std::io::Result<()> {
    let mut img = image::load_from_memory(bytes).unwrap();
    let (height, width) = img.dimensions();

    let image_box_size = std::cmp::min(height, width);

    let mut cx = 0; //top-left x coordinate of crop box
    let mut cy = 0; //top-left y coordinate of crop box

    if height > width {
        cx = (height - image_box_size) / 2;
    } else if width > height {
        cy = (width - image_box_size) / 2;
    }

    img
        .crop(cx, cy, image_box_size, image_box_size)   //croping center of image
        .thumbnail(100, 100) //downscaling
        .save(format!("./{}/preview_{}.jpg", IMAGE_FOLDER, &name))
}

fn get_image_ext_from_response(res: &reqwest::r#async::Response) -> Result<String, Error> {
    match res.headers().get(reqwest::header::CONTENT_TYPE) {
        Some(content_type) => {
            let content_type = Mime::from_str(content_type.to_str().unwrap()).unwrap();
            if content_type.type_() != mime::IMAGE{
                return Err(error::ErrorInternalServerError("File is not an image"));
            }

            match content_type.subtype() {
                mime::PNG => Ok("png".to_string()),
                mime::BMP => Ok("bmp".to_string()),
                mime::GIF => Ok("gif".to_string()),
                _ => Ok("jpg".to_string())  //convert to jpg if extension was not found
            }
        },
        None => Err(error::ErrorInternalServerError("Request doesn't contain a content-type header"))
    }
}

//downloading an image
fn get_data_from_url(url: String) -> impl Future<Item = String, Error = Error> {
    Client::new()
        .get(&url)
        .send()
        .and_then(|res| {
            let ext = get_image_ext_from_response(&res).unwrap();
            res.into_body().concat2().map(|body| (
                body,
                ext,
            )).from_err()
        })
        .map(|(body, ext)| handle_image_from_url(body, ext))
        .map_err(|e| {
            println!("get_data_from_url failed: {}", e);
            error::ErrorInternalServerError(e)
        })        
}

//saving downloaded image
fn handle_image_from_url(body: Chunk, ext: String) -> String {
    let filename = generate_filename();
    let filename_ext = format!("{}.{}", filename, ext);
    let filepath = format!("./{}/{}", IMAGE_FOLDER, filename_ext);
    let buf = body.into_buf();

    //running blocking fs method in thread pool
    web::block(move || {
        let mut file = fs::File::create(&filepath).unwrap();
        let data: Vec<u8> = buf.collect();
        file.write_all(&data).unwrap();
        generate_preview(&data, &filename).unwrap();
        Ok(filename_ext)
    }).map_err(|e: error::BlockingError<std::io::Error>| {
        match e {
            error::BlockingError::Error(e) => error::ErrorInternalServerError(e),
            error::BlockingError::Canceled => error::ErrorInternalServerError("File upload canceled"),
        }
    }).wait().unwrap()
}

//method for json upload (url: /img, method: post, content-type: application/json)
fn upload_json(req: web::Json<UploadImageJson>) -> impl Future<Item = HttpResponse, Error = Error> {
    stream::iter_ok::<Vec<String>, Error>(req.urls.to_vec())
        .map(|url| get_data_from_url(url))
        .buffer_unordered(PARALLEL_REQUESTS)
        .collect()
        .map(|filenames| {
            if filenames.len() > 0 {
                HttpResponse::Ok().json(filenames)
            } else {
                HttpResponse::BadRequest().finish()
            }
            
        })
        .map_err(|_| {
            println!("upload_json failed");
            error::ErrorInternalServerError("upload_json failed")
        })
}


//method for multipart upload (url: /img, method: post, content-type: multipart/form-data)
fn upload(multipart: Multipart) -> impl Future<Item = HttpResponse, Error = Error> {
    multipart
        .map_err(error::ErrorInternalServerError)
        .map(|field| handle_image_multipart(field).into_stream())
        .flatten()
        .collect()
        .map(|filenames| HttpResponse::Ok().json(filenames))
        .map_err(|e| {
            println!("upload failed: {}", e);
            e
        })
}

//test page (url: /img, method: get)
fn index() -> HttpResponse {
    let html = r#"
    <!doctype html>
    <html>
        <head><title>Upload Test</title></head>
        <body>
            <form action="/upload" method="post" enctype="multipart/form-data">
                <input type="file" name="file[]" multiple/>
                <input type="submit" value="Submit"></button>
            </form>
        </body>
    </html>"#;

    HttpResponse::Ok().body(html)
}

//content-length predicate
fn check_content_length(req: &RequestHead, max_size: &usize) -> bool {
    if req.method == actix_web::http::Method::GET {
        return true;
    }

    match req.headers().get("content-length") {
        Some(content_length_value) => {
            let content_length = content_length_value
                                    .to_str()
                                    .unwrap()
                                    .to_string()
                                    .parse::<usize>()
                                    .unwrap();

            content_length < *max_size
        },
        None => false
    }
}

//content-type predicate
fn is_multipart(req: &RequestHead) -> bool {
    match req.headers().get("content-type") {
        Some(content_type) => {
        content_type.to_str()
            .unwrap()
            .contains("multipart/form-data")
        },
        None => false
    }
}

fn main() -> std::io::Result<()> {
    std::env::set_var("RUST_LOG", "actix_server=info,actix_web=info");
    env_logger::init();
    fs::create_dir_all("./imgs").expect("Can't create an image directory");

    HttpServer::new(|| {
        App::new()
            .wrap(middleware::Logger::default())
            .service(
                web::resource("/")
                    .route(web::get().to(index))
            )
            .service(
                web::resource("/upload")
                    .guard(guard::fn_guard(|req| check_content_length(req, &(MAX_CONTENT_LENGTH * 1024 * 1024))))
                    .route(web::post()
                        .guard(guard::Post())
                        .guard(
                            guard::fn_guard(|req| is_multipart(req)))
                        .to_async(upload)
                    )
                    .route(web::route()
                        .guard(guard::Post())
                        .guard(guard::Header("content-type", "application/json"))
                        .to_async(upload_json)
                    ) 
            )
    })
    .bind("0.0.0.0:8080")?
    .run()
}

#[cfg(test)]
mod tests {
    extern crate http;

    use actix_web::http::{StatusCode};
    use actix_web::test;
    use super::*;

    #[test]
    fn test_upload_json() {
        let payload = web::Json(UploadImageJson{
            urls: vec![
                "http://via.placeholder.com/150x150".to_string(),
                "http://via.placeholder.com/150x200".to_string()
            ]
        });

        fs::create_dir_all("./imgs").expect("Can't create an image directory");

        let res = test::block_on(upload_json(payload)).unwrap();
        assert_eq!(res.status(), StatusCode::OK);


        let payload = web::Json(UploadImageJson{
            urls: vec![]
        });

        let res = test::block_on(upload_json(payload)).unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_get_image_ext_from_response() {
        let res = reqwest::r#async::Response::from(
            http::Response::builder()
                .header("content-type", "image/bmp")
                .status(StatusCode::OK)
                .body("test")
                .unwrap()
        );

        let ext = get_image_ext_from_response(&res).unwrap();
        
        assert_eq!(ext, "bmp");
    }

    #[test]
    fn test_get_file_ext_from_path() {
        assert_eq!(get_file_ext_from_path("example.com/filename.jpg"), "jpg");
    }
}