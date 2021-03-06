extern crate image;
extern crate img_hash;
extern crate postgres;
#[macro_use] extern crate log;
extern crate log4rs;
extern crate time;
#[macro_use] extern crate hyper;
extern crate url;
extern crate toml;

use std::env;
use postgres::{Connection, SslMode};
use img_hash::{ImageHash, HashType};
use std::thread;
use std::default::Default;
use time::{Timespec, Duration};
use hyper::Client;
use hyper::header::ContentType;
use image::ImageFormat;
use image::GenericImage;
use std::io::Read;
use hyper::status::StatusCode;
use postgres::stmt::Statement;
use std::fs::File;
use url::form_urlencoded;

header! { (XExtra, "X-Extra") => [String] }
header! { (XStream, "X-Stream") => [String] }

fn prepare_statement<'a>(conn: &'a Connection, stmt: &str) -> Statement<'a> {
    loop {
        let prep_stmt = conn.prepare_cached(stmt);
        if prep_stmt.is_err() {
            error!("Error in prepare for '{}': {}",
                   stmt,
                   prep_stmt.unwrap_err());
            thread::sleep(Duration::seconds(4).to_std().unwrap());
            continue;
        }
        return prep_stmt.unwrap();
    }
}

fn get_motion(old_hash: postgres::Result<String>, new_hash: &img_hash::ImageHash) -> f64 {
    match old_hash {
        Ok(val) => {
            match ImageHash::from_base64(&val) {
                Ok(val) => val.dist_ratio(new_hash) as f64,
                Err(_) => -1 as f64
            }
        },
        Err(_) => -1 as f64
    }
}

fn get_config() -> toml::Value {
    let mut config_string = String::new();
    File::open("config.toml").unwrap().read_to_string(&mut config_string).unwrap();
    let mut parser = toml::Parser::new(&config_string);

    parser.parse().unwrap().get("config").unwrap().clone()
}

fn main() {
    let config_table = get_config();
    let interval = Duration::minutes(config_table.lookup("refresh_minutes")
                                                 .unwrap()
                                                 .as_integer()
                                                 .unwrap());
    let imager_url: &str = &(String::from(config_table.lookup("imager_host")
                                                      .unwrap()
                                                      .as_str()
                                                      .unwrap()) +
                             &String::from("/streams"));
    let check_ms = Duration::milliseconds(
        config_table.lookup("check_ms").unwrap().as_integer().unwrap())
        .to_std().unwrap();

    log4rs::init_file("log.toml", Default::default()).unwrap();
    let db_url: &str = &env::var("DATABASE_URL").unwrap();
    let conn = Connection::connect(db_url, SslMode::None).unwrap();

    let query_stmt = prepare_statement(&conn,
                                       "select * from videos_video order by \"lastRetrieved\" \
                                        asc limit 1");
    let update_stmt = prepare_statement(&conn,
                                        "update videos_video set working = true, hash = $1, \
                                         motion = $2, \"lastRetrieved\" = $3, extra = $4, \"streamURL\" = $5 where id = $6");
    let not_working_stmt = prepare_statement(&conn,
                                     "update videos_video set working = false, motion = 0, \"lastRetrieved\" = $1 where id = $2");

    info!("Connected to Postgres and ready to update video...");
    loop {
        let res = query_stmt.query(&[]);
        if res.is_err() {
            error!("Error in query: {}", res.unwrap_err());
            thread::sleep(check_ms);
            continue;
        }

        let items = res.unwrap();
        if items.is_empty() {
            warn!("No video feeds");
            thread::sleep(check_ms);
            continue;
        }
        let item = items.get(0);
        let url: String = item.get("url");
        let when: postgres::Result<Timespec> = item.get_opt("lastRetrieved").unwrap();
        let id: i32 = item.get("id");
        let old_hash: postgres::Result<String> = item.get_opt("hash").unwrap();

        if when.is_err() {
            debug!("Item: {}, When: never", url);
        } else {
            let now = time::now().to_timespec();
            let diff: Duration = now - when.unwrap();
            debug!("Item: {}, When: {}", url, diff);
            if diff < interval {
                debug!("oldest item is young: {}", diff);
                thread::sleep(check_ms);
                continue;
            }
        }
        info!("Updating {}", url);

        let data: String = form_urlencoded::Serializer::new(String::new())
            .append_pair("url", &url)
            .finish();

        let raw_resp = Client::new()
                           .post(imager_url)
                           .header(ContentType::form_url_encoded())
                           .body(&data)
                           .send();

        if raw_resp.is_err() {
            warn!("{:?}", raw_resp.unwrap_err());
            warn!("Can't connect to imager");
            thread::sleep(check_ms);
            continue;
        }

        let mut resp = raw_resp.unwrap();

        let now = time::now().to_timespec();
        if resp.status != StatusCode::Ok {
            warn!("{:?}", resp.status_raw());
            not_working_stmt.execute(&[&now, &id]).unwrap();
            thread::sleep(check_ms);
            continue;
        }

        let extra: String = resp.headers.get::<XExtra>().map_or("{}".to_string(), |v| v.to_string());
        let stream: String = resp.headers.get::<XStream>().map_or("".to_string(), |v| v.to_string());

        let mut buf = Vec::new();
        resp.read_to_end(&mut buf).unwrap();
        let image = image::load_from_memory_with_format(&buf, ImageFormat::PNG).unwrap();
        let (width, height) = image.dimensions();
        if width == 0 || height == 0 {
            warn!("Dodgy image dimensions, skipping (width = {}, height = {})", width, height);
            not_working_stmt.execute(&[&now, &id]).unwrap();
            thread::sleep(check_ms);
            continue;
        }
        let hash = ImageHash::hash(&image, 16, HashType::DoubleGradient);

        debug!("Image hash: {}", hash.to_base64());
        let motion = get_motion(old_hash, &hash);
        debug!("Difference {}", motion);
        match update_stmt.execute(&[&hash.to_base64(), &motion, &now, &extra, &stream, &id]) {
            Ok(_) => info!("Updated {} with motion {}", url, motion),
            Err(err) => warn!("Error executing update: {:?}", err),
        }
    }
}
