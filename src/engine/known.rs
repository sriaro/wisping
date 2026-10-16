use super::classify::{Advice, Role};
use super::startup::{StartupEntry, StartupKind};

use Advice::{Caution, Keep, Safe};
use Role::{App, Hardware, Microsoft, Security, Updater};

const SERVICES: &[(&str, Role, Advice)] = &[
    ("steam client service", App, Caution),
    ("microsoft edge update service", Updater, Safe),
    ("microsoft edge elevation service", Updater, Safe),
    ("edgeupdate", Updater, Safe),
    ("edgeupdatem", Updater, Safe),
    ("microsoftedgeelevationservice", Updater, Safe),
    ("mozilla maintenance service", Updater, Safe),
    ("google update service", Updater, Safe),
    ("gupdate", Updater, Safe),
    ("gupdatem", Updater, Safe),
    ("googlechromeelevationservice", Updater, Safe),
    ("adobe acrobat update service", Updater, Safe),
    ("adobearmservice", Updater, Safe),
    ("adobe genuine monitor service", App, Safe),
    ("adobe genuine software integrity service", App, Safe),
    ("onedrive updater service", Updater, Safe),
    ("dropbox update service", Updater, Safe),
    ("anticheatexpert protection", Security, Keep),
    ("easyanticheat", Security, Keep),
    ("easyanticheat_eos", Security, Keep),
    ("battleye service", Security, Keep),
    ("beservice", Security, Keep),
    ("vgc", Security, Keep),
    ("vgk", Security, Keep),
    ("nvidia display container ls", Hardware, Keep),
    ("nvdisplay.containerlocalsystem", Hardware, Keep),
    ("nvidia localsystem container", Hardware, Caution),
    ("nvcontainer", Hardware, Caution),
    ("nvidia frameview sdk service", Hardware, Safe),
    ("nvidia telemetry container", Hardware, Safe),
    ("nvidia networkservice container", Hardware, Safe),
    ("nvidia broadcast container", Hardware, Caution),
    ("amd external events utility", Hardware, Caution),
    ("amd crash defender service", Hardware, Safe),
    ("realtek audio universal service", Hardware, Keep),
    ("rtkaudiouniversalservice", Hardware, Keep),
    ("razer synapse service", Hardware, Caution),
    ("razer chroma sdk service", Hardware, Caution),
    ("logitech g hub updater service", Updater, Safe),
    ("intel(r) content protection heci service", Hardware, Caution),
    ("intel(r) dynamic application loader host interface service", Hardware, Caution),
    ("intel(r) driver & support assistant", Updater, Safe),
    ("vmware authorization service", App, Caution),
    ("vmware usb arbitration service", App, Caution),
    ("vmware nat service", App, Caution),
    ("docker desktop service", App, Caution),
    ("bonjour service", App, Caution),
    ("apple mobile device service", App, Caution),
    ("skype updater", Updater, Safe),
];

const STARTUP: &[(&str, Role, Advice)] = &[
    ("spotify", App, Safe),
    ("discord", App, Safe),
    ("steam", App, Safe),
    ("slack", App, Safe),
    ("zoom", App, Safe),
    ("zoomopener", App, Safe),
    ("dropbox", App, Safe),
    ("skype", App, Safe),
    ("google drive", App, Safe),
    ("googledrivefs", App, Safe),
    ("adobe creative cloud", App, Safe),
    ("creative cloud", App, Safe),
    ("ccxprocess", App, Safe),
    ("epicgameslauncher", App, Safe),
    ("battle.net", App, Safe),
    ("ubisoft connect", App, Safe),
    ("gog galaxy", App, Safe),
    ("ea desktop", App, Safe),
    ("ituneshelper", App, Safe),
    ("itunes helper", App, Safe),
    ("onedrive", Microsoft, Safe),
    ("microsoft teams", Microsoft, Safe),
    ("teams", Microsoft, Safe),
    ("microsoft edge", Microsoft, Safe),
    ("microsoft.windowsterminal", Microsoft, Safe),
    ("lghub", Hardware, Caution),
    ("logitech g hub", Hardware, Caution),
    ("razer synapse", Hardware, Caution),
    ("rtkngui64", Hardware, Caution),
    ("realtek hd audio manager", Hardware, Caution),
    ("nvbackend", Hardware, Safe),
    ("nvidia geforce experience", Hardware, Caution),
    ("securityhealth", Security, Keep),
    ("windows security notification icon", Security, Keep),
];

fn keys(name: &str) -> Vec<String> {
    let lower = name.to_lowercase();
    let mut out = vec![lower.clone()];
    let mut parts: Vec<&str> = lower.split(" / ").collect();
    if parts.len() > 1 {
        parts.iter().for_each(|p| out.push(p.trim().to_string()));
    }
    for p in std::mem::take(&mut parts) {
        if let Some((base, rest)) = p.split_once(" (") {
            out.push(base.trim().to_string());
            out.push(rest.trim_end_matches(')').trim().to_string());
        }
    }
    out
}

pub fn lookup(e: &StartupEntry) -> Option<(Role, Advice)> {
    let table = if e.kind == StartupKind::Service { SERVICES } else { STARTUP };
    let keys = keys(&e.name);
    table.iter().find(|(name, _, _)| keys.iter().any(|k| k == name)).map(|(_, r, a)| (*r, *a))
}
