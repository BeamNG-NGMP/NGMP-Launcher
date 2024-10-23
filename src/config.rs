use serde::{Serialize, Deserialize};

pub fn load_config() -> Config {
    match std::fs::read_to_string("client_config.toml") {
        Ok(content) => {
            match toml::from_str::<Config>(&content) {
                Ok(config) => return config,
                Err(e) => panic!("failed to parse client_config.toml: {}", e),
            }
        },
        Err(e) => panic!("failed to read client_config.toml: {}", e),
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    #[serde(rename = "Networking")]
    pub networking: ConfigNetworking,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ConfigNetworking {
    pub launcher_port: u16,
    pub http_port: u16,
}
