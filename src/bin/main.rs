#[macro_use]
extern crate serde_json;
extern crate wikibase;

use config::{Config, File};
use serde_json::Value;
use std::collections::HashMap;
use std::error::Error;
use wikibase::entity_container::*;
use wikibase::mediawiki::api::{Api, NamespaceID};
use wikibase::mediawiki::title::Title;
use wikibase::*;

#[derive(Debug, Clone)]
pub struct MW {
    pub api: Api,
    pub ec: EntityContainer,
}

impl MW {
    pub fn new(api_url: &str) -> Self {
        let mut ret = Self {
            api: Api::new(api_url).expect("MediaWikiAPI new failed"),
            ec: EntityContainer::new(),
        };
        ret.api.set_edit_delay(Some(500)); // 500 ms delay after each edit
        ret.ec.allow_special_entity_data(false);
        ret
    }

    pub fn api_query_prop2(
        &self,
        key1: &str,
        value1: &str,
        key2: &str,
        value2: &String,
    ) -> Result<Value, Box<dyn Error>> {
        let params: HashMap<String, String> =
            vec![("action", "query"), (key1, value1), (key2, value2.as_str())]
                .iter()
                .map(|x| (x.0.to_string(), x.1.to_string()))
                .collect();
        self.api.get_query_api_json(&params)
    }

    pub fn get_page_id(&self, title: &Title) -> Result<NamespaceID, Box<dyn Error>> {
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
            return match page_id.parse::<NamespaceID>() {
                Ok(ret) => {
                    if ret < 0 {
                        Err(From::from("Page does not exist"))
                    } else {
                        Ok(ret)
                    }
                }
                Err(_) => Err(From::from("Can't parse NamespaceID")),
            };
        }
        Err(From::from(format!(
            "get_page_id: No page ID in JSON: {}",
            res
        )))
    }

    pub fn load_entity<S: Into<String>>(&mut self, entity_id: S) -> Result<Entity, Box<dyn Error>> {
        self.ec.load_entity(&self.api, entity_id)
    }

    pub fn wbcreateclaim(
        self: &mut Self,
        entity: &String,
        snaktype: wikibase::SnakType,
        valuetype: &str,
        property: &String,
        value: &wikibase::Value,
        summary: Option<String>,
        baserevid: Option<u64>,
        rank: Option<&str>,
    ) -> Result<Value, Box<dyn Error>> {
        let rank = match rank {
            Some(s) => s,
            None => "normal",
        }
        .to_string();

        let mut params: HashMap<String, String> = HashMap::new();

        let j = json!({"claims":[{"mainsnak":{"snaktype":snaktype,"property":property,"datavalue":{"value":value,"type":valuetype}},"type":"statement","rank":rank}]});
        let j = ::serde_json::to_string(&j).expect("MW::wbcreateclaim: json::to_string failed");

        params.insert("action".to_string(), "wbeditentity".to_string());
        params.insert("id".to_string(), entity.to_string());
        params.insert("data".to_string(), j);
        /*
        params.insert("snaktype".to_string(), snaktype);
        params.insert("property".to_string(), property.to_string());
        params.insert("value".to_string(), value);
        */
        self.add_summary(&mut params, summary);
        self.add_baserevid(&mut params, baserevid);
        self.add_bot_flag(&mut params);
        self.add_edit_token(&mut params)?;

        if false {
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
    ) -> Result<(), Box<dyn Error>> {
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

    pub fn add_target_prominent(
        &mut self,
        source_item: &String,
        filename: &String,
        property: &String,
    ) -> Result<(), Box<dyn Error>> {
        let new_value =
            wikibase::Value::Entity(EntityValue::new(EntityType::Item, source_item.clone()));

        let title = Title::new(&filename, 6);
        let page_id = match self.get_page_id(&title) {
            Ok(id) => id,
            Err(_) => {
                return Err(From::from(format!(
                    "Could not get page ID for File:{}",
                    &filename
                )))
            }
        };
        let media_id = format!("M{}", page_id);
        //println!("Media ID for {} is {}", title.pretty(), &media_id);

        // Check if this item already has this statement
        let has_statement: bool = match self.load_entity(media_id.clone()) {
            Ok(mi) => mi
                .claims_with_property(property.clone())
                .iter()
                .any(|statement| match statement.main_snak().data_value() {
                    Some(dv) => *dv.value() == new_value,
                    None => false,
                }),
            Err(_) => false,
        };

        if has_statement {
            //println!("Already has a statement for {}", &property);
            return Ok(());
        }

        match self.wbcreateclaim(
            &media_id,
            SnakType::Value,
            "wikibase-entityid",
            &property,
            &new_value,
            Some(format!(
                "Used with P18 on Wikidata [[:d:{}|]] #rust_commons_statement",
                &source_item
            )),
            None,
            Some("preferred"),
        ) {
            Ok(_) => {}
            Err(e) => eprintln!("Error editing: {:?}", e),
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct CategoryItemImage {
    pub category: String,
    pub item: Option<String>,
    pub image: Option<String>,
}

fn main() {
    let mut settings = Config::default();
    settings.merge(File::with_name("bot.ini")).unwrap();
    let lgname = settings.get_str("user.user").unwrap();
    let lgpass = settings.get_str("user.pass").unwrap();

    let mut commons = MW::new("https://commons.wikimedia.org/w/api.php");
    commons.api.set_edit_delay(Some(500)); // Half a second between edits
    commons.api.login(lgname, lgpass).unwrap();

    let url = "https://petscan.wmflabs.org/?psid=11247873&format=json"; // TODO FIXME
    let petscan_result = commons
        .api
        .query_raw(url, &commons.api.no_params(), "GET")
        .expect("Petscan query failed");
    let petscan_result: Value =
        serde_json::from_str(&petscan_result).expect("JSON parsing of PetScan result failed");

    let categories = match petscan_result["*"][0]["a"]["*"].as_array() {
        Some(c) => c,
        None => panic!("PetScan query failed"),
    };
    let mut cii: Vec<CategoryItemImage> = categories
        .iter()
        .filter_map(|c| match (c["title"].as_str(), c["q"].as_str()) {
            (Some(title), Some(q)) => Some(CategoryItemImage {
                category: title.to_string(),
                item: Some(q.to_string()),
                image: None,
            }),
            _ => None,
        })
        .collect();

    // Load entities
    commons
        .ec
        .load_entities(
            &commons.api,
            &cii.iter()
                .map(|c| c.item.as_ref().unwrap().clone())
                .collect(),
        )
        .expect("Loading of entities failed [1]");

    // Category item => main topic
    let mut to_load: Vec<String> = vec![];
    cii.iter_mut().for_each(|c| {
        let entity = match commons.ec.get_entity(c.item.as_ref().unwrap()) {
            Some(e) => e,
            None => {
                c.item = None;
                return;
            }
        };
        if !entity.has_target_entity("P31", "Q4167836") {
            return;
        }
        match entity.values_for_property("P301").iter().nth(0) {
            Some(target) => {
                //println!("{} => {:?}", c.item.as_ref().unwrap(), target);
                match target {
                    wikibase::Value::Entity(e) => {
                        c.item = Some(e.id().to_string());
                        to_load.push(e.id().to_string());
                    }
                    _ => c.item = None,
                }
            }
            None => c.item = None,
        }
    });
    cii.retain(|c| c.item.is_some());

    // Load remaining items
    commons
        .ec
        .load_entities(&commons.api, &to_load)
        .expect("Loading of entities failed [2]");

    // Get images
    cii.iter_mut().for_each(|c| {
        c.image = match commons.ec.get_entity(c.item.as_ref().unwrap().to_owned()) {
            Some(item) => item
                .values_for_property("P18")
                .iter()
                .filter_map(|i| match i {
                    wikibase::Value::StringValue(s) => Some(s.to_owned()),
                    _ => None,
                })
                .nth(0),
            None => return,
        };
    });
    cii.retain(|c| c.item.is_some() && c.image.is_some());

    // Paranoia
    cii.retain(|c| match commons.ec.get_entity(c.item.as_ref().unwrap()) {
        Some(entity) => !entity.has_target_entity("P31", "Q4167836"),
        None => false,
    });

    // Add "depicts" to files
    cii.iter().for_each(|c| {
        println!("{:?}", &c);
        match (c.item.as_ref(), c.image.as_ref()) {
            (Some(item), Some(image)) => {
                match commons.add_target_prominent(&item, &image, &"P180".to_string()) {
                    Ok(_) => {}
                    Err(e) => eprintln!("{:?} : {:?}", c, e),
                }
            }
            _ => {}
        }
    });

    //println!("{:#?}", &cii);

    /*
        let source_item = "Q62378".to_string();
        let filename = "Tower of London viewed from the River Thames.jpg".to_string();
        let property = "P180".to_string();

        println!("{}", &result);
    */
}
