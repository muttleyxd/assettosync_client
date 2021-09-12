/*use compress_tools::list_archive_files;
use fs_extra::dir::CopyOptions;
use gtk::prelude::*;
use std::error;
use std::fs::{self, DirEntry};
use std::io;
use std::{fs::File, path::Path};
use tempdir::TempDir;
use unrar::archive;
use walkdir::WalkDir;

mod common;
mod install_task;

fn install_archive(archive_path: &str, assetto_path: &str) -> compress_tools::Result<()> {
    let temp_dir = TempDir::new("/tmp/assetto_sync_unpack")?;
    let temporary_directory = temp_dir.path();
    common::unpack_archive(Path::new(archive_path), temporary_directory)?;
    for task in
        install_task::determine_install_tasks(&common::recursive_ls(temporary_directory)).unwrap()
    {
        let target_path = Path::new(assetto_path).join(task.target_path);
        println!("{} -> {}", task.source_path, target_path.display().to_string());
        let result = fs_extra::dir::move_dir(task.source_path, target_path, &CopyOptions::new());
        if let Err(error) = result {
            println!("error installing mod: {}", error.to_string());
        }
    }
    Ok(())
}*/

use glib::{glib_sys::gboolean, translate::FromGlibPtrNone};
use install_thread::InstallThreadTrait;
use reqwest::Client;
// These require the `serde` dependency.
use serde::{Deserialize, Serialize};
use std::{
    array,
    convert::TryInto,
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::task::JoinHandle;

mod common;
mod config;
mod install_task;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonModTemplate {
    pub checksum_md5: String,
    pub filename: String,
    pub size_in_bytes: u64,
}

use config::{ConfigObject, ConfigTrait};
use gtk::{prelude::*, DialogExt, WidgetExt, *};
use scopeguard::guard;

fn is_valid_assetto_path(path: &Path) -> bool {
    return path.join("acs.exe").exists();
}

fn get_assetto_path(existing_path: &String) -> Result<String, String> {
    if is_valid_assetto_path(Path::new(existing_path)) {
        return Ok(existing_path.clone());
    }

    let dialog = FileChooserDialog::with_buttons::<Window>(
        Some("Pick Assetto Corsa Home directory"),
        None,
        FileChooserAction::SelectFolder,
        &[
            ("_Cancel", ResponseType::Cancel),
            ("_Open", ResponseType::Accept),
        ],
    );
    let guard = guard(dialog, |dialog| {
        dialog.hide();
    });
    if guard.run() == ResponseType::Accept {
        let result = guard.get_filename().unwrap();

        return match is_valid_assetto_path(result.as_path()) {
            true => Ok(result.to_string_lossy().to_string()),
            false => Err(format!("Path {:?} does not contain acs.exe", result)),
        };
    }
    Err("No path provided".to_string())
}

use std::fs::File;
use std::io;

const LOGIN_LINK: &str = "https://acsync.team8.pl/login";
const MODS_JSON_LINK: &str = "https://acsync.team8.pl/mods.json";

struct LoginData {
    login: String,
    password: String,
}

fn login_dialog() -> Result<LoginData, String> {
    let glade_src = include_str!("login.glade");
    let builder = gtk::Builder::new();
    let result = builder.add_from_string(glade_src);
    if let Err(error) = result {
        panic!("failed to parse login.glade: {}", error);
    }

    let dialog: gtk::Dialog = builder.get_object("dialog").unwrap();
    if dialog.run() == ResponseType::Cancel {
        return Err("User canceled dialog".to_string());
    }
    dialog.hide();

    let tb_login: gtk::Entry = builder.get_object("tb_login").unwrap();
    let tb_password: gtk::Entry = builder.get_object("tb_password").unwrap();

    Ok(LoginData {
        login: tb_login.get_text().to_string(),
        password: tb_password.get_text().to_string(),
    })
}

async fn login(login: &String, password: &String) -> Result<(LoginData, Client), String> {
    let mut login_data = LoginData {
        login: login.clone(),
        password: password.clone(),
    };
    if login_data.login.is_empty() {
        let dialog_data = login_dialog();
        if let Err(error) = dialog_data {
            return Err(error);
        }
        login_data = dialog_data.unwrap();
    }

    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::custom(|attempt| attempt.stop()))
        .build();
    if let Err(error) = client {
        return Err(error.to_string());
    }

    let client = client.unwrap();

    let response = client
        .post(LOGIN_LINK)
        .form(&[
            ("login", &login_data.login),
            ("password", &login_data.password),
        ])
        .send()
        .await;

    if let Err(error) = response {
        return Err(error.to_string());
    }

    let response = response.unwrap();
    let has_user_name_cookie = response.cookies().any(|c| c.name() == "user_name");

    match has_user_name_cookie {
        true => Ok((login_data, client)),
        false => Err("Login failed (wrong password?)".to_string()),
    }
}

async fn get_mod_list(client: &Client) -> Result<Vec<JsonModTemplate>, reqwest::Error> {
    let mod_list: Vec<JsonModTemplate> = client.get(MODS_JSON_LINK).send().await?.json().await?;
    Ok(mod_list)
}

fn fill_mod_list(
    lv_mods_store: Arc<Mutex<gtk::ListStore>>,
    config: &ConfigObject,
    mod_list: &Vec<JsonModTemplate>,
) {
    for entry in mod_list.iter() {
        let enabled = config.is_mod_installed(&entry.checksum_md5);
        let size_str = format!("{}M", entry.size_in_bytes / 1024 / 1024);
        lv_mods_store.lock().unwrap().insert_with_values(
            None,
            &[0, 1, 2],
            &[&enabled, &entry.filename, &size_str],
        );
    }
}

fn get_task_list(
    lv_mods_store: Arc<Mutex<gtk::ListStore>>,
    config: &mut ConfigObject,
    mod_list: &Vec<JsonModTemplate>,
) -> Vec<JsonModTemplate> {
    let mut task_list = vec![];
    let store = lv_mods_store.lock().unwrap();
    for (index, entry) in mod_list.iter().enumerate() {
        let iter = store
            .get_iter_from_string(index.to_string().as_str())
            .unwrap();
        let should_install = store.get_value(&iter, 0).get::<bool>().unwrap().unwrap();

        if !should_install {
            continue;
        }
        if config.is_mod_installed(&entry.checksum_md5) {
            continue;
        }

        task_list.push(entry.clone());
    }
    task_list
}

fn display_summary(summary: &String) {
    let glade_src = include_str!("summary.glade");
    let builder = gtk::Builder::new();
    let result = builder.add_from_string(glade_src);
    if let Err(error) = result {
        panic!("failed to parse summary.glade: {}", error);
    }

    let label_summary: gtk::Label = builder.get_object("label_summary").unwrap();
    label_summary.set_text(summary);

    /*let button_ok: gtk::Button = builder.get_object("button_ok").unwrap();
    button_ok.connect_clicked(move |_| {
        println!("hai");
        gtk::main_quit();
    });*/

    let dialog: gtk::Dialog = builder.get_object("dialog").unwrap();
    let _ = dialog.run();
    dialog.hide();
}

mod install_thread;

/* Download file:
    let mut resp = client.get("http://localhost:8000/mod_management/download?hash=ddf7cb7a8dd889f3de6b649624a02725").send().await?;
    let mut out = File::create("out.rar").unwrap();
    let result = io::copy(&mut resp.bytes().await.unwrap().as_ref(), &mut out);
*/
async fn install_mods(
    client: Client,
    lv_mods_store: Arc<Mutex<gtk::ListStore>>,
    config: &mut ConfigObject,
    mod_list: &Vec<JsonModTemplate>,
) {
    let glade_src = include_str!("worker.glade");
    let builder = gtk::Builder::new();
    let result = builder.add_from_string(glade_src);
    if let Err(error) = result {
        panic!("failed to parse main.glade: {}", error);
    }

    let task_list = get_task_list(lv_mods_store, config, mod_list);
    let install_thread = Arc::new(Mutex::new(install_thread::InstallThread::new(
        client, task_list,
    )));
    let assetto_path = config.config.assetto_path.clone();
    let install_thread_clone = install_thread.clone();
    let mut task = tokio::spawn(async move {
        let result: Option<JoinHandle<()>>;
        {
            let mut install_thread = install_thread_clone.lock().unwrap();
            result = Some(install_thread.start(assetto_path));
        }
        let _ = result.unwrap().await;
    });

    let window: gtk::Window = builder.get_object("window1").unwrap();

    let label_status: gtk::Label = builder.get_object("label_status").unwrap();

    let install_thread_clone = install_thread.clone();
    glib::timeout_add_local(100, move || {
        let install_thread = install_thread_clone.lock().unwrap();
        label_status.set_text(install_thread.get_status().as_str());

        if install_thread.is_finished() {
            gtk::main_quit();
            Continue(false)
        } else {
            Continue(true)
        }
    });

    window.show_all();

    gtk::main();
    window.hide();

    let install_thread = install_thread.lock().unwrap();

    let successfully_installed_mods = install_thread.get_successfully_installed_mods();
    for checksum in successfully_installed_mods.iter() {
        config.add_installed_mod(checksum);
    }

    let error_list = install_thread.get_error_list();
    if error_list.len() == 0 {
        display_summary(&format!(
            "{} mods installed successfully.",
            successfully_installed_mods.len()
        ));
    } else {
        let mut summary = format!(
            "{} mods installed successfully.\nErrors:\n",
            successfully_installed_mods.len()
        );
        for error in error_list.iter() {
            summary += format!("{}\n", error).as_str();
        }
        display_summary(&summary);
    }

    let _ = task.await;
}

#[tokio::main]
async fn main() -> reqwest::Result<()> {
    if gtk::init().is_err() {
        println!("Failed to initialize GTK.");
        return Ok(());
    }

    let config_dir = dirs::config_dir().unwrap();
    let config_file = config_dir.join("assetto_sync_client.json");
    let mut config = config::ConfigObject::new(config_file.to_str().unwrap());

    let mut assetto_path = get_assetto_path(&config.config.assetto_path);
    while let Err(error) = &assetto_path {
        let dialog = MessageDialog::new(
            None::<&Window>,
            DialogFlags::MODAL,
            MessageType::Error,
            ButtonsType::YesNo,
            format!(
                "{}.\nDo you want to try again?\n\"No\" will close the program.",
                error
            )
            .as_str(),
        );
        if dialog.run() == ResponseType::No {
            println!("Error: {}", error.to_string());
            return Ok(());
        }
        assetto_path = get_assetto_path(&config.config.assetto_path);
    }

    config.set_assetto_path(assetto_path.unwrap());

    let mut login_data = login(&config.config.login, &config.config.password).await;
    while let Err(error) = login_data {
        if error == "User canceled dialog" {
            println!("Closing: {}", error);
            return Ok(());
        }
        login_data = login(&"".to_string(), &"".to_string()).await;
    }

    let login_data = login_data.unwrap();
    let client = login_data.1;
    let login_data = login_data.0;

    config.set_login(login_data.login);
    config.set_password(login_data.password);

    let mod_list = get_mod_list(&client).await;
    if let Err(error) = mod_list {
        println!("Error receiving mods: {}", error.to_string());
        return Ok(());
    }
    let mod_list = mod_list.unwrap();

    let glade_src = include_str!("main.glade");
    let builder = gtk::Builder::new();
    let result = builder.add_from_string(glade_src);
    if let Err(error) = result {
        panic!("failed to parse main.glade: {}", error);
    }

    let lv_mods_store: Arc<Mutex<gtk::ListStore>> =
        Arc::new(Mutex::new(builder.get_object("lv_mods_store").unwrap()));
    let lv_mods_toggle_box: gtk::CellRendererToggle =
        builder.get_object("lv_mods_toggle_box").unwrap();
    fill_mod_list(lv_mods_store.clone(), &config, &mod_list);

    let toggle_box_store = lv_mods_store.clone();
    lv_mods_toggle_box.connect_toggled(move |_, path| {
        let store = toggle_box_store.lock().unwrap();
        let row = store.get_iter(&path).unwrap();

        let old_value = store.get_value(&row, 0);
        let new_value = (!old_value.get::<bool>().unwrap().unwrap()).to_value();
        store.set_value(&row, 0, &new_value);
    });

    let window: Arc<Mutex<gtk::Window>> =
        Arc::new(Mutex::new(builder.get_object("window1").unwrap()));

    let window_event_clone = window.clone();
    window_event_clone
        .lock()
        .unwrap()
        .connect_delete_event(|wind, _| {
            gtk::main_quit();
            Inhibit(false)
        });

    let install_selected = Arc::new(Mutex::new(false));

    let window_button_install = window.clone();
    let button_install: gtk::Button = builder.get_object("button_install").unwrap();
    let install_selected_clone = install_selected.clone();
    button_install.connect_clicked(move |_| {
        let window = window_button_install.lock().unwrap();
        window.set_visible(false);
        *install_selected_clone.lock().unwrap() = true;
        gtk::main_quit();
    });

    let button_cancel: gtk::Button = builder.get_object("button_cancel").unwrap();
    let window_button_cancel = window.clone();
    button_cancel.connect_clicked(move |_| {
        let window = window_button_cancel.lock().unwrap();
        window.set_visible(false);
        gtk::main_quit();
    });

    window.lock().unwrap().show_all();
    gtk::main();

    if *install_selected.lock().unwrap() == false {
        println!("Cancel clicked");
        return Ok(());
    }

    install_mods(client, lv_mods_store.clone(), &mut config, &mod_list).await;

    /* Get mods:
        let mods: Vec<JsonModTemplate> = client.get("http://localhost:8000/mods.json").send().await?.json().await?;
        println!("mods: {:?}", mods);
    */
    /* Download file:
        let mut resp = client.get("http://localhost:8000/mod_management/download?hash=ddf7cb7a8dd889f3de6b649624a02725").send().await?;
        let mut out = File::create("out.rar").unwrap();
        let result = io::copy(&mut resp.bytes().await.unwrap().as_ref(), &mut out);
    */
    Ok(())
}
/*let archive_path =
    "/home/muttley/git/assettosync/work/mod94fullv12.7z".to_string();
install_archive(archive_path.as_str(), "/tmp/ac_install")?;
Ok(())*/

/*
    if gtk::init().is_err() {
        println!("Failed to initialize GTK.");
        return;
    }
    let glade_src = include_str!("main.glade");
    let builder = gtk::Builder::new();
    let result = builder.add_from_string(glade_src);
    if let Err(error) = result {
        panic!("failed to parse main.glade: {}", error);
    }

    let window: gtk::Window = builder.get_object("window1").unwrap();

    let button: gtk::Button = builder.get_object("button_one").unwrap();

    button.connect_clicked(move |_| {
        println!("Clicked");
    });

    window.show_all();

    gtk::main();

}*/
