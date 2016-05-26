use hyper;
use webdriver::command::{GetParameters, LocatorParameters};
use webdriver::common::LocatorStrategy;
use webdriver::error::{WebDriverResult, ErrorStatus, WebDriverError};
use webdriver::response::{WebDriverResponse, ValueResponse};
use std::io::Read;
use std::ops::Drop;

use rustc_serialize::json;
use rustc_serialize::json::Json;
use rustc_serialize::json::ToJson;
use rustc_serialize::base64::FromBase64Error;
use rustc_serialize::base64::FromBase64;

pub struct Webdriver {
    client: hyper::client::Client,
}

pub struct WebdriverSession<'a> {
    webdriver: &'a Webdriver,
    session_id: String,
}

fn decode_response(json_str: &str) -> WebDriverResult<Json> {
    let decoded = Json::from_str(&json_str).unwrap();
    let status =
        decoded.find("status").expect("status code").as_u64().expect("numeric status code");
    let value = decoded.find("value").expect("value");
    return match status {
        0 => Ok(decoded.clone()),
        _ => {
            let message = value.find("message")
                .expect("error message")
                .as_string()
                .expect("a string message")
                .to_string();
            let kind = match status {
                7 => ErrorStatus::NoSuchElement,
                _ => ErrorStatus::UnknownError,
            };
            return Err(WebDriverError::new(kind, message));
        }
    };
}

trait DoesPost<'a> {
    fn do_post(&'a self, url: &str, body: &String) -> WebDriverResult<Json> {
        debug!("Request: {:?}", body);
        let mut res = self.client()
            .post(url)
            .body(body)
            .send()
            .unwrap();
        let mut buffer = String::new();
        res.read_to_string(&mut buffer).unwrap();
        debug!("Buffer: {}", buffer);
        // assert_eq!(res.status, hyper::Ok);
        let decoded = decode_response(&buffer);
        debug!("Decoded: {:?}", decoded);
        return decoded;
    }

    fn client(&'a self) -> &'a hyper::client::Client;
}

impl Webdriver {
    pub fn new() -> Webdriver {
        Webdriver { client: hyper::client::Client::new() }
    }

    pub fn make_session(&self) -> WebdriverSession {
        let mut mobile_emulation: json::Object = json::Object::new();
        mobile_emulation.insert("deviceName".to_string(), "Apple iPhone 5".to_json());
        let mut chrome_options: json::Object = json::Object::new();
        chrome_options.insert("mobileEmulation".to_string(), mobile_emulation.to_json());
        let mut desired: json::Object = json::Object::new();
        desired.insert("chromeOptions".to_string(), chrome_options.to_json());
        let mut request: json::Object = json::Object::new();
        request.insert("desiredCapabilities".to_string(), desired.to_json());
        let json_str = (&request).to_json().to_string();
        let decoded = self.do_post("http://localhost:9516/session", &json_str);
        let session_id = decoded.expect("ok response")
            .find("sessionId")
            .expect("sessionId")
            .as_string()
            .expect("string session id")
            .to_string();
        WebdriverSession {
            webdriver: self,
            session_id: session_id,
        }
    }
}

impl<'a> DoesPost<'a> for Webdriver {
    fn client(&'a self) -> &'a hyper::client::Client {
        &self.client
    }
}
impl<'a> DoesPost<'a> for WebdriverSession<'a> {
    fn client(&'a self) -> &'a hyper::client::Client {
        &self.webdriver.client()
    }
}

impl <'a>Drop for WebdriverSession<'a> {
    fn drop(&mut self) {
        self.client()
            .delete(&format!("http://localhost:9516/session/{}", self.session_id))
            .send()
            .unwrap();
    }
}

impl <'a>WebdriverSession<'a> {
    pub fn goto_url(&self, url: String) {
        let params = GetParameters { url: url };
        self.client()
            .post(&format!("http://localhost:9516/session/{}/url", self.session_id))
            .body(&params.to_json().to_string())
            .send()
            .unwrap();
    }

    pub fn find_element_by_xpath(&self, xpath: String) -> WebDriverResult<WebDriverResponse> {
        let params = LocatorParameters {
            value: xpath.clone(),
            using: LocatorStrategy::XPath,
        };
        let decoded =
            self.do_post(&format!("http://localhost:9516/session/{}/element", self.session_id),
                         &params.to_json().to_string());
        return match decoded {
            Err(val) => Err(val),
            Ok(val) => {
                Ok(WebDriverResponse::Generic(ValueResponse::new(val.find("value")
                    .expect("has value")
                    .clone())))
            }
        };
    }

    fn get_for_element(&self, element: &ValueResponse, kind: &str) -> WebDriverResult<Json> {
        let element_id =
            element.value.find("ELEMENT").expect("ELEMENT").as_string().expect("String ELEMENT");
        let mut res = self.client()
            .get(&format!("http://localhost:9516/session/{}/element/{}/{}",
                          self.session_id,
                          element_id,
                          kind))
            .send()
            .unwrap();
        let mut buffer = String::new();
        res.read_to_string(&mut buffer).unwrap();
        debug!("Buffer: {}", buffer);
        // assert_eq!(res.status, hyper::Ok);
        let decoded = decode_response(&buffer);
        debug!("Decoded: {:?}", decoded);
        return decoded;
    }

    pub fn get_element_location(&self, element: &ValueResponse) -> WebDriverResult<Json> {
        return self.get_for_element(element, "location");
    }

    pub fn get_element_size(&self, element: &ValueResponse) -> WebDriverResult<Json> {
        return self.get_for_element(element, "size");
    }

    pub fn get_screenshot_as_png(&self) -> Result<Vec<u8>, FromBase64Error> {
        let mut res = self.client()
            .get(&format!("http://localhost:9516/session/{}/screenshot",
                          self.session_id))
            .send()
            .unwrap();
        let mut buffer = String::new();
        res.read_to_string(&mut buffer).unwrap();
        let decoded = decode_response(&buffer).expect("decoded");
        let value = decoded.find("value").expect("value").as_string().expect("string value");
        return value.clone().from_base64();
    }
}
