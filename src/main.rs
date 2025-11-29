#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

// --- СТРУКТУРИ ДАНИХ (API) ---

#[derive(Deserialize, Debug, Clone)]
struct ApiAttributes {
    name: String,
    players: u32,
    #[serde(rename = "maxPlayers")]
    max_players: u32,
    details: ApiDetails,
    country: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct ApiDetails {
    map: Option<String>,
    #[serde(rename = "gameMode")]
    game_mode: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct ApiServerData {
    attributes: ApiAttributes,
}

#[derive(Deserialize, Debug, Clone)]
struct ApiLinks {
    next: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct ApiResponse {
    data: Vec<ApiServerData>,
    links: Option<ApiLinks>,
}

// --- СТРУКТУРИ ДЛЯ GUI ---

#[derive(Clone, Debug)]
struct ServerItem {
    name: String,
    players: u32,
    max_players: u32,
    map: String,
    mode: String,
    country: String,
}

#[derive(Clone, Debug)]
struct ScanResult {
    servers: Vec<ServerItem>,
    next_url: String,
}

#[derive(Deserialize, Serialize, PartialEq, Clone)]
enum Language {
    En,
    Ua,
}

// --- ЛОГІКА ДОДАТКУ ---

#[derive(Deserialize, Serialize)]
#[serde(default)]
struct SquadApp {
    min_players: u32,
    max_players: u32,
    banned_countries: HashSet<String>,
    filter_name: String, // НОВЕ ПОЛЕ
    filter_map: String,
    filter_mode: String,
    language: Language,

    #[serde(skip)]
    servers: Vec<ServerItem>,
    #[serde(skip)]
    next_url: String,
    #[serde(skip)]
    show_settings: bool,
    #[serde(skip)]
    rx: Option<Receiver<ScanResult>>,
    #[serde(skip)]
    is_loading: bool,
    #[serde(skip)]
    first_load_done: bool,
}

impl Default for SquadApp {
    fn default() -> Self {
        let mut banned = HashSet::new();
        for code in ["RU", "BY", "CN", "BR"] {
            banned.insert(code.to_string());
        }

        Self {
            servers: Vec::new(),
            min_players: 0,
            max_players: 100,
            banned_countries: banned,
            filter_name: String::new(),
            filter_map: String::new(),
            filter_mode: String::new(),
            language: Language::En,
            
            next_url: String::new(),
            show_settings: false,
            rx: None,
            is_loading: false,
            first_load_done: false,
        }
    }
}

fn fetch_servers(
    min_p: u32, 
    max_p: u32, 
    banned: HashSet<String>, 
    f_name: String, // НОВИЙ АРГУМЕНТ
    f_map: String, 
    f_mode: String, 
    override_url: String
) -> ScanResult {
    
    let client = reqwest::blocking::Client::new();
    let mut final_servers = Vec::new();
    let mut next_link = String::new();
    
    let ban_words_ru = ["RUSSIA", "MOSCOW", "SPB", "USSR", "ZOV", "WAGNER", "[RU]"];
    let ban_words_cn = ["CHINESE", "ASIA", "[CN]", "QQ", "DOUYU"];

    let is_infinite_scroll = !override_url.is_empty();
    
    let mut current_url = if is_infinite_scroll {
        override_url
    } else {
        "https://api.battlemetrics.com/servers?filter[game]=squad&filter[status]=online&page[size]=100&sort=-players".to_string()
    };

    let pages_to_fetch = if is_infinite_scroll { 1 } else { 3 };

    for _ in 0..pages_to_fetch {
        let mut request = client.get(&current_url);
        
        if !is_infinite_scroll {
            request = request
                .query(&[("filter[players][min]", min_p.to_string())])
                .query(&[("filter[players][max]", max_p.to_string())]);
        }

        match request.send() {
            Ok(resp) => {
                if let Ok(json) = resp.json::<ApiResponse>() {
                    if let Some(links) = json.links {
                        if let Some(nxt) = links.next {
                            next_link = nxt;
                            current_url = next_link.clone();
                        } else {
                            next_link = "".to_string();
                        }
                    }

                    for server_data in json.data {
                        let attr = server_data.attributes;
                        let country = attr.country.unwrap_or("??".to_string());
                        let name = attr.name;
                        let players = attr.players;
                        let max_players = attr.max_players;
                        let map = attr.details.map.unwrap_or("Unknown".to_string());
                        let mode = attr.details.game_mode.unwrap_or("Unknown".to_string());
                        
                        let mut skip = false;
                        if country != "UA" {
                            if banned.contains(&country) { skip = true; }
                            
                            let name_upper = name.to_uppercase();
                            if banned.contains("RU") {
                                for w in ban_words_ru { if name_upper.contains(w) { skip = true; break; } }
                            }
                            if banned.contains("CN") {
                                for w in ban_words_cn { if name_upper.contains(w) { skip = true; break; } }
                            }
                        }
                        if skip { continue; }

                        // НОВИЙ ФІЛЬТР
                        if !f_name.is_empty() && !name.to_lowercase().contains(&f_name.to_lowercase()) { continue; }
                        
                        if !f_map.is_empty() && !map.to_lowercase().contains(&f_map.to_lowercase()) { continue; }
                        if !f_mode.is_empty() && !mode.to_lowercase().contains(&f_mode.to_lowercase()) { continue; }

                        let clean_name = if name.len() > 48 { format!("{}...", &name[..45]) } else { name };

                        final_servers.push(ServerItem {
                            name: clean_name,
                            players,
                            max_players,
                            map,
                            mode,
                            country,
                        });
                    }
                } else {
                    break;
                }
            },
            Err(_) => break,
        }
        
        if next_link.is_empty() { break; }
    }

    ScanResult {
        servers: final_servers,
        next_url: next_link,
    }
}

impl SquadApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        if let Some(storage) = cc.storage {
            return eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default();
        }
        Default::default()
    }

    fn tr(&self, key: &str) -> String {
        match (key, &self.language) {
            ("app_title", Language::En) => "Squad Browser".to_owned(),
            ("app_title", Language::Ua) => "Пошук Серверів Squad".to_owned(),
            ("settings", Language::En) => "Settings".to_owned(),
            ("settings", Language::Ua) => "Налаштування".to_owned(),
            ("start", Language::En) => "START SCAN".to_owned(),
            ("start", Language::Ua) => "ПОЧАТИ ПОШУК".to_owned(),
            ("refresh", Language::En) => "REFRESH".to_owned(),
            ("refresh", Language::Ua) => "ОНОВИТИ".to_owned(),
            ("found", Language::En) => "Servers:".to_owned(),
            ("found", Language::Ua) => "Серверів:".to_owned(),
            ("no_servers", Language::En) => "No servers found.".to_owned(),
            ("no_servers", Language::Ua) => "Серверів не знайдено.".to_owned(),
            ("conf_title", Language::En) => "Configuration".to_owned(),
            ("conf_title", Language::Ua) => "Конфігурація".to_owned(),
            ("min_p", Language::En) => "Min Players:".to_owned(),
            ("min_p", Language::Ua) => "Мін. Гравців:".to_owned(),
            ("max_p", Language::En) => "Max Players:".to_owned(),
            ("max_p", Language::Ua) => "Макс. Гравців:".to_owned(),
            ("search_name", Language::En) => "Server Name:".to_owned(), // НОВЕ
            ("search_name", Language::Ua) => "Назва Сервера:".to_owned(), // НОВЕ
            ("map", Language::En) => "Map Name:".to_owned(),
            ("map", Language::Ua) => "Назва Карти:".to_owned(),
            ("mode", Language::En) => "Game Mode:".to_owned(),
            ("mode", Language::Ua) => "Режим Гри:".to_owned(),
            ("close", Language::En) => "Close & Save".to_owned(),
            ("close", Language::Ua) => "Зберегти і Закрити".to_owned(),
            ("lang", Language::En) => "Language:".to_owned(),
            ("lang", Language::Ua) => "Мова:".to_owned(),
            ("bl_title", Language::En) => "🚫 Disabled Countries".to_owned(),
            ("bl_title", Language::Ua) => "🚫 Заблоковані Країни".to_owned(),
            ("scanning", Language::En) => "Scanning...".to_owned(),
            ("scanning", Language::Ua) => "Пошук...".to_owned(),
            ("loading_more", Language::En) => "Loading more...".to_owned(),
            ("loading_more", Language::Ua) => "Підвантажую ще...".to_owned(),
            ("ready", Language::En) => "Ready".to_owned(),
            ("ready", Language::Ua) => "Готовий".to_owned(),
            _ => key.to_owned(),
        }
    }

    fn run_scan(&mut self, next_page_url: Option<String>) {
        if self.is_loading { return; }

        self.is_loading = true;
        
        if next_page_url.is_none() {
            self.servers.clear();
        }
        
        let (tx, rx): (Sender<ScanResult>, Receiver<ScanResult>) = channel();
        self.rx = Some(rx);

        let min_p = self.min_players;
        let max_p = self.max_players;
        let banned = self.banned_countries.clone();
        let f_name = self.filter_name.clone(); // ПЕРЕДАЄМО
        let f_map = self.filter_map.clone();
        let f_mode = self.filter_mode.clone();
        let url_arg = next_page_url.unwrap_or_default();

        thread::spawn(move || {
            let result = fetch_servers(min_p, max_p, banned, f_name, f_map, f_mode, url_arg);
            let _ = tx.send(result);
        });
    }
}

impl eframe::App for SquadApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(rx) = &self.rx {
            if let Ok(response) = rx.try_recv() {
                self.servers.extend(response.servers);
                self.next_url = response.next_url;
                self.is_loading = false;
                self.first_load_done = true;
                self.rx = None;
            }
        }

        let mut trigger_load_more_url: Option<String> = None;
        let mut trigger_new_scan = false;

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(self.tr("app_title"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(format!("⚙ {}", self.tr("settings"))).clicked() {
                        self.show_settings = !self.show_settings;
                    }
                });
            });
            
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                let btn_size = [140.0, 40.0];
                if !self.first_load_done {
                     if ui.add_sized(btn_size, egui::Button::new(self.tr("start"))).clicked() {
                        trigger_new_scan = true;
                    }
                } else {
                    if ui.add_sized(btn_size, egui::Button::new(self.tr("refresh"))).clicked() {
                        trigger_new_scan = true;
                    }
                }
                
                if self.is_loading {
                    ui.spinner();
                }

                let status_msg = if self.is_loading {
                    if self.servers.is_empty() { self.tr("scanning") } else { self.tr("loading_more") }
                } else if self.first_load_done {
                    format!("{} {}", self.tr("found"), self.servers.len())
                } else {
                    "".to_owned()
                };
                ui.label(status_msg);
            });

            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                if self.servers.is_empty() && self.first_load_done {
                    ui.label(self.tr("no_servers"));
                }

                let total_servers = self.servers.len();

                for (index, server) in self.servers.iter().enumerate() {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.colored_label(egui::Color32::from_rgb(255, 165, 0), format!("[{}]", server.country));
                            ui.colored_label(egui::Color32::LIGHT_BLUE, &server.name);
                        });
                        ui.horizontal(|ui| {
                            ui.label(format!("{} | {}", server.map, server.mode));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let color = if server.players >= server.max_players - 2 { egui::Color32::RED } else { egui::Color32::GREEN };
                                ui.colored_label(color, format!("{}/{}", server.players, server.max_players));
                            });
                        });
                    });

                    if index >= total_servers.saturating_sub(5) 
                       && !self.is_loading 
                       && !self.next_url.is_empty() 
                    {
                        trigger_load_more_url = Some(self.next_url.clone());
                    }
                }
                
                if self.is_loading && !self.servers.is_empty() {
                    ui.add_space(10.0);
                    ui.centered_and_justified(|ui| ui.spinner());
                }
            });
        });

        if trigger_new_scan {
            self.run_scan(None);
        }

        if let Some(url) = trigger_load_more_url {
            self.run_scan(Some(url));
        }

        if self.show_settings {
            let mut open = true;
            let mut close_settings = false;

            egui::Window::new(self.tr("conf_title"))
                .open(&mut open)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(self.tr("lang"));
                        ui.selectable_value(&mut self.language, Language::En, "English");
                        ui.selectable_value(&mut self.language, Language::Ua, "Українська");
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(self.tr("min_p"));
                        ui.add(egui::Slider::new(&mut self.min_players, 0..=100));
                    });
                    ui.horizontal(|ui| {
                        ui.label(self.tr("max_p"));
                        ui.add(egui::Slider::new(&mut self.max_players, 0..=100));
                    });
                    ui.separator();
                    ui.collapsing(self.tr("bl_title"), |ui| {
                        egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                            let countries = vec![
                                ("RU", "Russia"), ("BY", "Belarus"), 
                                ("CN", "China"), ("BR", "Brazil"), 
                                ("AU", "Australia"), ("SG", "Singapore"), 
                                ("KZ", "Kazakhstan"), ("HK", "Hong Kong"),
                                ("TR", "Turkey"), ("US", "USA"), ("CA", "Canada")
                            ];
                            for (code, name) in countries {
                                let mut is_banned = self.banned_countries.contains(code);
                                if ui.checkbox(&mut is_banned, format!("{} ({})", code, name)).changed() {
                                    if is_banned {
                                        self.banned_countries.insert(code.to_string());
                                    } else {
                                        self.banned_countries.remove(code);
                                    }
                                }
                            }
                        });
                    });
                    ui.separator();
                    
                    // ДОДАВ ФІЛЬТР ПО ІМЕНІ
                    ui.horizontal(|ui| {
                        ui.label(self.tr("search_name"));
                        ui.text_edit_singleline(&mut self.filter_name);
                    });

                    ui.horizontal(|ui| {
                        ui.label(self.tr("map"));
                        ui.text_edit_singleline(&mut self.filter_map);
                    });
                    ui.horizontal(|ui| {
                        ui.label(self.tr("mode"));
                        ui.text_edit_singleline(&mut self.filter_mode);
                    });
                    ui.add_space(10.0);
                    if ui.button(self.tr("close")).clicked() {
                        close_settings = true;
                    }
                });
            
            if close_settings || !open {
                self.show_settings = false;
            }
        }
    }
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([650.0, 850.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Squad Browser",
        options,
        Box::new(|cc| Box::new(SquadApp::new(cc))),
    )
}