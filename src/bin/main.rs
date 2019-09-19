#[macro_use]
extern crate serde_json;
extern crate reqwest;
extern crate wikibase;

//use config::{Config, File};
use percent_encoding::percent_decode;
use serde_json::Value;
use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::prelude::*;
use std::io::BufReader;
use std::time::Duration;
use wikibase::entity_container::*;
use wikibase::mediawiki::api::{Api, NamespaceID};
use wikibase::mediawiki::title::Title;
use wikibase::*;

#[derive(Debug, Clone)]
pub struct MW {
    pub api: Api,
    pub ec: EntityContainer,
    pub bot_log_file: String,
    pub verbose: bool,
}

impl MW {
    pub fn new(api_url: &str) -> Self {
        let mut ret = Self {
            api: Api::new_from_builder(api_url, Self::get_builder())
                .expect("MediaWikiAPI new failed"),
            ec: EntityContainer::new(),
            bot_log_file: "bot.log".to_string(),
            verbose: false,
        };
        ret.api.set_edit_delay(Some(500)); // 500 ms delay after each edit
        ret.ec.allow_special_entity_data(false);
        ret
    }

    fn get_builder() -> reqwest::ClientBuilder {
        reqwest::ClientBuilder::new().timeout(Duration::from_secs(240))
    }

    pub fn new_from_ini_file(filename: &str, api_url: &str) -> Self {
        let mut settings = config::Config::default();
        settings.merge(config::File::with_name(filename)).unwrap();
        let lgname = settings.get_str("user.user").unwrap();
        let lgpass = settings.get_str("user.pass").unwrap();

        let mut ret = Self::new(api_url);
        ret.api.set_edit_delay(Some(500)); // Half a second between edits
        ret.api.login(lgname, lgpass).unwrap();
        ret
    }

    pub fn in_bot_log(&self, parts: Vec<&String>) -> bool {
        let f = match File::open(self.bot_log_file.to_owned()) {
            Ok(f) => f,
            _ => return false,
        };
        let parts: Vec<String> = parts.iter().map(|s| format!("\"{}\"", s)).collect(); // Quote parts
        let f = BufReader::new(f);
        let ret = f
            .lines()
            .filter_map(|l| l.ok())
            .any(|l| parts.iter().all(|p| l.contains(p)));
        if self.verbose && ret {
            println!("Found a row for {:?}", &parts);
        }
        ret
    }

    pub fn append_log(&self, line: String) {
        if self.verbose {
            println!("{:?}", &line);
        }
        let mut file = OpenOptions::new()
            .write(true)
            .append(true)
            .open(self.bot_log_file.to_owned())
            .unwrap();
        match writeln!(file, "{}", line) {
            _ => {} // Meh
        }
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

    pub fn get_free_page_image(&self, mw_api: &Api, page: &String) -> Option<String> {
        mw_api
            .get_query_api_json(&mw_api.params_into(&vec![
                ("action", "query"),
                ("prop", "pageprops"),
                ("titles", page.as_str()),
            ]))
            .ok()?["query"]["pages"]
            .as_object()?
            .iter()
            .filter_map(|(_pageid, pagedata)| pagedata["pageprops"]["page_image_free"].as_str())
            .map(|s| s.to_string())
            .nth(0)
    }

    pub fn page_contains_template(&self, page: &String, template: &str) -> bool {
        match self.api.get_query_api_json(&self.api.params_into(&vec![
            ("action", "query"),
            ("prop", "templates"),
            ("tltemplates", format!("Template:{}", template).as_str()),
            ("titles", page.as_str()),
        ])) {
            Ok(j) => match j["query"]["pages"].as_object() {
                Some(pages) => pages
                    .iter()
                    .any(|(_pageid, pagedata)| pagedata["templates"].is_array()),
                None => false,
            },

            _ => false,
        }
    }

    // file with File: prefix!
    pub fn is_artwork(&self, file: &String) -> bool {
        self.page_contains_template(file, "Artwork")
    }

    pub fn percent_decode_title(s: String) -> String {
        percent_decode(s.as_bytes())
            .decode_utf8()
            .expect(format!("fix_attribute_value: '{}' is not utf8", s).as_str())
            .replace(' ', "_")
            .to_string()
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

fn _depicts_german_buildings() {
    let mut commons = MW::new_from_ini_file("bot.ini", "https://commons.wikimedia.org/w/api.php");
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
        .filter(|c| !commons.is_artwork(&c.image.as_ref().unwrap()))
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

    // Remove ones we had already
    cii.retain(|c| {
        !commons.in_bot_log(vec![&c.item.as_ref().unwrap(), &c.image.as_ref().unwrap()])
    });

    // Paranoia
    cii.retain(|c| match commons.ec.get_entity(c.item.as_ref().unwrap()) {
        Some(entity) => !entity.has_target_entity("P31", "Q4167836"),
        None => false,
    });

    // Add "depicts" to files
    cii.iter()
        .for_each(|c| match (c.item.as_ref(), c.image.as_ref()) {
            (Some(item), Some(image)) => {
                commons.append_log(format!("Adding \"P180\": \"{}\" to \"{}\"", &item, &image));
                match commons.add_target_prominent(&item, &image, &"P180".to_string()) {
                    Ok(_) => {}
                    Err(e) => eprintln!("{:?} : {:?}", c, e),
                }
            }
            _ => {}
        });
}

//________________________________________________________________________________________________________________

#[derive(Debug, Clone)]
struct ItemArticleImagesPageImage {
    pub q: String,
    pub article: String,
    pub p18: Option<String>,
    pub pageimage: Option<String>,
}

fn depicts_p18_and_free_page_image(sparql_part: &str, server: &str) {
    let mut commons = MW::new_from_ini_file("bot.ini", "https://commons.wikimedia.org/w/api.php");
    let local_wiki_api = Api::new_from_builder(
        format!("https://{}/w/api.php", &server).as_str(),
        MW::get_builder(),
    )
    .unwrap();
    let sparql = format!("SELECT ?q ?image ?article {{ {} . ?q  wdt:P18 ?image . ?article schema:about ?q ; schema:isPartOf <https://{}/> }}",&sparql_part,&server);
    let wikidata =
        Api::new_from_builder("https://www.wikidata.org/w/api.php", MW::get_builder()).unwrap();
    let json = wikidata.sparql_query(&sparql).expect("SPARQL query failed");

    let iaipi: Vec<ItemArticleImagesPageImage> = match json["results"]["bindings"].as_array() {
        Some(b) => b,
        None => panic!("No bindings in SPARQL results"),
    }
    .iter()
    .filter_map(|b| {
        let (q, p18, article) = match (
            b["q"]["value"].as_str(),
            b["image"]["value"].as_str(),
            b["article"]["value"].as_str(),
        ) {
            (Some(q), Some(i), Some(a)) => (q, i, a),
            _ => return None,
        };
        Some(ItemArticleImagesPageImage {
            q: wikidata.extract_entity_from_uri(q).ok()?,
            p18: Some(MW::percent_decode_title(p18.split('/').last()?.to_string())),
            article: MW::percent_decode_title(article.split('/').last()?.to_string()),
            pageimage: None,
        })
    })
    .filter(|i| !commons.in_bot_log(vec![&i.q, &i.p18.as_ref().unwrap()]))
    .collect();

    iaipi.iter().for_each(
        |x| match commons.get_free_page_image(&local_wiki_api, &x.article) {
            Some(pageimage) => {
                if x.p18 == Some(pageimage.to_owned()) {
                    if !commons.is_artwork(&format!("File:{}", &pageimage)) {
                        commons.append_log(format!("{:?} : \"{}\"", &x, &pageimage));
                        match commons.add_target_prominent(
                            &x.q,
                            &x.p18.as_ref().unwrap(),
                            &"P180".to_string(),
                        ) {
                            Ok(_) => {}
                            Err(e) => eprintln!("{:?} : {:?}", x, e),
                        }
                    }
                }
            }
            None => {}
        },
    );
}

fn main() {
    //depicts_german_buildings();
    depicts_p18_and_free_page_image("?q wdt:P31 wd:Q5 ; wdt:P21 wd:Q6581072", "de.wikipedia.org");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_artwork() {
        let commons = MW::new("https://commons.wikimedia.org/w/api.php");
        assert!(commons.is_artwork(
            &"File:Lady_Elizabeth_Hamilton_(1753–1797),_Countess_of_Derby.jpg".to_string()
        ));
        assert!(!commons.is_artwork(
            &"File:09797jfBarangays_West_Triangle_Quezon_City_Avenue_Bridgefvf_02.jpg".to_string()
        ));
    }
}
