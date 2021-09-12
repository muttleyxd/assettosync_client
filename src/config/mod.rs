use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    pub assetto_path: String,
    pub login: String,
    pub installed_mods_md5: Vec<String>,
    pub password: String,
}

pub trait ConfigTrait {
    fn new(path: &str) -> Self;
    fn add_installed_mod(&mut self, md5: &String);
    fn is_mod_installed(&self, md5: &String) -> bool;
    fn set_assetto_path(&mut self, path: String);
    fn set_login(&mut self, login: String);
    fn set_password(&mut self, password: String);
}

pub struct ConfigObject {
    pub config: Config,
    pub path: String,
}

fn read_config(path: &str) -> String {
    let file = std::fs::read_to_string(path);
    match file {
        Ok(file) => file,
        Err(_) => serde_json::to_string(&Config::default()).unwrap(),
    }
}

fn write_config_to_json(path: &Path, config: &Config) {
    let json = serde_json::to_string_pretty(config);
    if let Ok(output) = json {
        std::fs::write(path, output).expect("Config file writing failure");
    }
}

impl ConfigTrait for ConfigObject {
    fn new(path: &str) -> ConfigObject {
        let content = read_config(path);
        let config: Config = serde_json::from_str(&content).unwrap();
        write_config_to_json(Path::new(path), &config);

        ConfigObject {
            config: config,
            path: path.to_string(),
        }
    }

    fn add_installed_mod(&mut self, md5: &String) {
        for checksum in self.config.installed_mods_md5.iter() {
            if *md5 == *checksum {
                return;
            }
        }
        self.config.installed_mods_md5.push(md5.clone());
        write_config_to_json(Path::new(&self.path), &self.config);
    }

    fn is_mod_installed(&self, md5: &String) -> bool {
        for checksum in self.config.installed_mods_md5.iter() {
            if *md5 == *checksum {
                return true;
            }
        }
        false
    }

    fn set_assetto_path(&mut self, path: String) {
        self.config.assetto_path = path;
        write_config_to_json(Path::new(&self.path), &self.config);
    }

    fn set_login(&mut self, login: String) {
        self.config.login = login;
        write_config_to_json(Path::new(&self.path), &self.config);
    }

    fn set_password(&mut self, password: String) {
        self.config.password = password;
        write_config_to_json(Path::new(&self.path), &self.config);
    }
}
