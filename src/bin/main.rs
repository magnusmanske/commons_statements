//#[macro_use]
//extern crate lazy_static;
//#[macro_use]
extern crate mediawiki;
//extern crate regex;
//extern crate reqwest;
#[macro_use]
extern crate serde_json;
extern crate wikibase;

use config::{Config, File};
use serde_json::Value;
use std::collections::HashMap;
use wikibase::entity_container::*;
use wikibase::*;
/*
//use multimap::MultiMap;
//use std::{thread, time};
use chrono::Local;
use percent_encoding::percent_decode;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::env;
use std::{error::Error, fmt};
use wikibase::entity_diff::*;
*/

pub struct MW {
    pub api: mediawiki::api::Api,
    pub ec: EntityContainer,
}

impl MW {
    pub fn new(api_url: &str) -> Self {
        Self {
            api: mediawiki::api::Api::new(api_url).expect("MediaWikiAPI new failed"),
            ec: EntityContainer::new(),
        }
    }

    pub fn api_query_prop2(
        &self,
        key1: &str,
        value1: &str,
        key2: &str,
        value2: &String,
    ) -> Result<Value, Box<::std::error::Error>> {
        let params: HashMap<String, String> =
            vec![("action", "query"), (key1, value1), (key2, value2.as_str())]
                .iter()
                .map(|x| (x.0.to_string(), x.1.to_string()))
                .collect();
        self.api.get_query_api_json(&params)
    }

    pub fn get_page_id(
        &self,
        title: &mediawiki::title::Title,
    ) -> Result<mediawiki::api::NamespaceID, Box<::std::error::Error>> {
        let res = self.api_query_prop2(
            "prop",
            "pageprops",
            "titles",
            &title
                .full_with_underscores(&self.api)
                .ok_or(format!("No namespace for title {:?}", &title))?,
        )?;
        let pages = res["query"]["pages"]
            .as_object()
            .ok_or(format!("get_page_id: No object.pages in JSON: {}", res))?;
        for (page_id, _page) in pages {
            return match page_id.parse::<mediawiki::api::NamespaceID>() {
                Ok(ret) => {
                    if ret < 0 {
                        Err(From::from("Page does not exist"))
                    } else {
                        Ok(ret)
                    }
                }
                Err(_) => Err(From::from("Can't parse mediawiki::api::NamespaceID")),
            };
        }
        Err(From::from(format!(
            "get_page_id: No page ID in JSON: {}",
            res
        )))
    }

    pub fn load_entity<S: Into<String>>(
        &mut self,
        entity_id: S,
    ) -> Result<&Entity, Box<::std::error::Error>> {
        self.ec.load_entity(&self.api, entity_id)
    }

    pub fn wbcreateclaim(
        self: &mut Self,
        entity: &String,
        snaktype: wikibase::SnakType,
        property: &String,
        value: &wikibase::Value,
        summary: Option<String>,
        baserevid: Option<u64>,
    ) -> Result<Value, Box<::std::error::Error>> {
        let snaktype = json!(snaktype);
        let snaktype = snaktype.as_str().unwrap().to_string();
        let value = json!(value);
        let value = serde_json::to_string(&value).unwrap();
        let mut params: HashMap<String, String> = HashMap::new();
        params.insert("action".to_string(), "wbcreateclaim".to_string());
        params.insert("entity".to_string(), entity.to_string());
        params.insert("snaktype".to_string(), snaktype);
        params.insert("property".to_string(), property.to_string());
        params.insert("value".to_string(), value);
        self.add_summary(&mut params, summary);
        self.add_baserevid(&mut params, baserevid);
        self.add_bot_flag(&mut params);
        self.add_edit_token(&mut params)?;

        if true {
            Ok(json!(params))
        } else {
            let ret = self.api.post_query_api_json_mut(&params);
            if ret.is_ok() {
                self.ec.remove_entity(entity.to_owned());
            }
            ret
        }
    }

    fn add_edit_token(
        self: &mut Self,
        params: &mut HashMap<String, String>,
    ) -> Result<(), Box<::std::error::Error>> {
        params.insert("token".to_string(), self.api.get_edit_token()?);
        Ok(())
    }

    fn add_bot_flag(&self, params: &mut HashMap<String, String>) {
        if self.api.user().is_bot() {
            params.insert("bot".to_string(), "1".to_string());
        }
    }

    fn add_baserevid(&self, params: &mut HashMap<String, String>, baserevid: Option<u64>) {
        match baserevid {
            Some(baserevid) => {
                params.insert("baserevid".to_string(), baserevid.to_string());
            }
            None => {}
        }
    }

    fn add_summary(&self, params: &mut HashMap<String, String>, summary: Option<String>) {
        match summary {
            Some(s) => {
                params.insert("summary".to_string(), s);
            }
            None => {}
        }
    }
}

fn main() {
    let mut settings = Config::default();
    settings.merge(File::with_name("bot.ini")).unwrap();
    let lgname = settings.get_str("user.user").unwrap();
    let lgpass = settings.get_str("user.pass").unwrap();

    let mut commons = MW::new("https://commons.wikimedia.org/w/api.php");
    commons.api.set_edit_delay(Some(500)); // Half a second between edits
    commons.api.login(lgname, lgpass).unwrap();

    let source_item = "Q62378".to_string();
    let filename = "Tower of London viewed from the River Thames.jpg".to_string();
    let property = "P180".to_string();

    let new_value =
        wikibase::Value::Entity(EntityValue::new(EntityType::Item, source_item.clone()));

    let title = mediawiki::title::Title::new(&filename, 6);
    let page_id = match commons.get_page_id(&title) {
        Ok(id) => id,
        Err(_) => return,
    };
    let media_id = format!("M{}", page_id);
    println!("Media ID for {} is {}", title.pretty(), &media_id);

    // Check if this item already has this statement
    let has_statement: bool = match commons.load_entity(media_id.clone()) {
        Ok(mi) => mi
            .claims_with_property(property.clone())
            .iter()
            .any(|statement| match statement.main_snak().data_value() {
                Some(dv) => *dv.value() == new_value,
                None => false,
            }),
        Err(_) => false,
    };
    println!("Has statement: {}", has_statement);
    if has_statement {
        return;
    }

    let result = commons
        .wbcreateclaim(
            &media_id,
            SnakType::Value,
            &property,
            &new_value,
            Some("Used with P18 on Wikidata".to_string()),
            None,
        )
        .unwrap();
    println!("{}", &result);
}
