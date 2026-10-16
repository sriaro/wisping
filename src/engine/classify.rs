use super::startup::{StartupEntry, StartupKind};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Windows,
    Security,
    Hardware,
    Updater,
    Microsoft,
    Menu,
    App,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Advice {
    Safe,
    Caution,
    Keep,
}

impl Role {
    pub fn key(self) -> &'static str {
        match self {
            Role::Windows => "windows",
            Role::Security => "security",
            Role::Hardware => "hardware",
            Role::Updater => "updater",
            Role::Microsoft => "microsoft",
            Role::Menu => "menu",
            Role::App => "app",
        }
    }
}

impl Advice {
    pub fn key(self) -> &'static str {
        match self {
            Advice::Safe => "safe",
            Advice::Caution => "caution",
            Advice::Keep => "keep",
        }
    }

    pub fn tone(self) -> i32 {
        match self {
            Advice::Safe => 0,
            Advice::Caution => 1,
            Advice::Keep => 2,
        }
    }
}

const SECURITY: &[&str] = &[
    "antivirus", "anti-virus", "defender", "anticheat", "anti-cheat", "easyanticheat", "battleye", "vanguard", "kaspersky", "norton", "mcafee", "avast", "avg ", "bitdefender", "malwarebytes", "eset", "firewall", "windows security", "securityhealth",
];

const HARDWARE: &[&str] = &[
    "nvidia", "advanced micro devices", "amd ", "intel", "realtek", "logitech", "razer", "corsair", "steelseries", "asus", "msi ", "micro-star", "gigabyte", "synaptics", "elan ", "dell ", "hewlett", "lenovo", "wacom", "bluetooth", "audio", "driver", "touchpad", "thunderbolt", "qualcomm", "mediatek", "nahimic",
];

const UPDATER: &[&str] = &["update", "updater", "maintenance", "elevation service", "autoupdate", "upgrade"];

fn has(text: &str, words: &[&str]) -> bool {
    words.iter().any(|w| text.contains(w))
}

pub fn classify(e: &StartupEntry) -> (Role, Advice) {
    if e.system {
        return (Role::Windows, Advice::Keep);
    }
    if e.kind == StartupKind::Context {
        return (Role::Menu, Advice::Safe);
    }
    if let Some(hit) = super::known::lookup(e) {
        return hit;
    }
    let service = e.kind == StartupKind::Service;
    let text = format!("{} {} {}", e.publisher, e.name, e.command).to_lowercase();
    if has(&text, SECURITY) {
        return (Role::Security, Advice::Keep);
    }
    if has(&e.name.to_lowercase(), UPDATER) {
        return (Role::Updater, Advice::Safe);
    }
    if has(&text, HARDWARE) {
        return (Role::Hardware, Advice::Caution);
    }
    if e.publisher.to_lowercase().contains("microsoft") {
        return (Role::Microsoft, if service { Advice::Caution } else { Advice::Safe });
    }
    (Role::App, if service { Advice::Caution } else { Advice::Safe })
}
