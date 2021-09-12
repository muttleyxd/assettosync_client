use std::{
    fs::File,
    io,
    path::Path,
    sync::{Arc, Mutex}
};

use reqwest::Client;
use tempdir::TempDir;

use crate::common;
use crate::install_task;
use crate::JsonModTemplate;

pub trait InstallThreadTrait {
    fn new(client: Client, task_list: Vec<JsonModTemplate>) -> Self;
    fn start(&mut self, assetto_path: String) -> tokio::task::JoinHandle<()>;
    fn get_error_list(&self) -> Vec<String>;
    fn get_status(&self) -> String;
    fn get_successfully_installed_mods(&self) -> Vec<String>;
    fn is_finished(&self) -> bool;
}

pub struct InstallThread {
    client: Arc<Mutex<Client>>,
    current_status: Arc<Mutex<String>>,
    error_list: Arc<Mutex<Vec<String>>>,
    is_finished: Arc<Mutex<bool>>,
    successful_mods_md5: Arc<Mutex<Vec<String>>>,
    task_list: Arc<Mutex<Vec<JsonModTemplate>>>,
}

const MOD_DOWNLOAD_LINK: &str = "https://acsync.team8.pl/mod_management/download?hash=";

fn get_download_link(md5_hash: &String) -> String {
    format!("{}{}", MOD_DOWNLOAD_LINK, md5_hash)
}

fn install_archive(archive_path: &str, assetto_path: &str) -> compress_tools::Result<()> {
    let temp_dir = TempDir::new("assetto_sync_unpack")?;
    let temporary_directory = temp_dir.path();
    common::unpack_archive(Path::new(archive_path), temporary_directory)?;
    for task in
        install_task::determine_install_tasks(&common::recursive_ls(temporary_directory)).unwrap()
    {
        let target_path = Path::new(assetto_path).join(task.target_path);
        println!(
            "{} -> {}",
            task.source_path,
            target_path.display().to_string()
        );
        let options = fs_extra::dir::CopyOptions {
            overwrite: true,
            skip_exist: false,
            buffer_size: 65536,
            copy_inside: false,
            content_only: false,
            depth: 0,
        };
        let result = fs_extra::dir::move_dir(task.source_path, target_path, &options);
        if let Err(error) = result {
            return Err(compress_tools::Error::from(error.to_string()));
        }
    }
    Ok(())
}

impl InstallThreadTrait for InstallThread {
    fn new(client: Client, task_list: Vec<JsonModTemplate>) -> InstallThread {
        InstallThread {
            client: Arc::new(Mutex::new(client)),
            current_status: Arc::new(Mutex::new("".to_string())),
            error_list: Arc::new(Mutex::new(vec![])),
            is_finished: Arc::new(Mutex::new(false)),
            successful_mods_md5: Arc::new(Mutex::new(vec![])),
            task_list: Arc::new(Mutex::new(task_list)),
        }
    }

    fn start(&mut self, assetto_path: String) -> tokio::task::JoinHandle<()> {
        let is_finished = self.is_finished.clone();
        let status_clone = self.current_status.clone();
        *self.current_status.lock().unwrap() = format!("starting workers");
        let error_list = self.error_list.clone();
        let successful_mods = self.successful_mods_md5.clone();

        let client = self.client.clone();
        let task_list = self.task_list.clone();

        tokio::task::spawn(async move {
            let download_dir = TempDir::new("assetto_sync_download");
            if let Err(error) = download_dir {
                error_list.lock().unwrap().push(format!(
                    "Cannot create download temporary dir, error: {}",
                    error.to_string()
                ));
                return;
            }
            let download_dir = download_dir.unwrap();
            let download_dir_path = download_dir.path();
            println!("Download dir path: {:?}", download_dir_path);

            let client = client.lock().unwrap().clone();
            let task_list = task_list.lock().unwrap().clone();
            for (index, task) in task_list.iter().enumerate() {
                *status_clone.lock().unwrap() = format!(
                    "Downloading mod {} ({}/{})",
                    task.filename,
                    index + 1,
                    task_list.len()
                );
                let link = get_download_link(&task.checksum_md5);
                let resp = client.get(link).send().await;
                if let Err(error) = resp {
                    error_list.lock().unwrap().push(format!(
                        "Mod {}, download error: {}",
                        task.filename,
                        error.to_string()
                    ));
                    continue;
                }

                *status_clone.lock().unwrap() = format!(
                    "Unpacking mod {} ({}/{})",
                    task.filename,
                    index + 1,
                    task_list.len()
                );
                let archive_path = download_dir_path.join(&task.filename);

                let resp = resp.unwrap();
                let mut out = File::create(&archive_path).unwrap();
                let result = io::copy(&mut resp.bytes().await.unwrap().as_ref(), &mut out);
                if let Err(error) = result {
                    error_list.lock().unwrap().push(format!(
                        "Mod {}, unpack error: {}",
                        task.filename,
                        error.to_string()
                    ));
                    continue;
                }
                let result = result.unwrap();
                if result != task.size_in_bytes {
                    error_list.lock().unwrap().push(format!(
                        "Mod {}, size mismatch (expected: {}, actual: {})",
                        task.filename, task.size_in_bytes, result
                    ));
                    continue;
                }

                *status_clone.lock().unwrap() = format!(
                    "Installing mod {} ({}/{})",
                    task.filename,
                    index + 1,
                    task_list.len()
                );
                let result = install_archive(
                    archive_path.to_str().unwrap(),
                    assetto_path.clone().as_str(),
                );
                if let Err(error) = result {
                    error_list.lock().unwrap().push(format!(
                        "Mod {}, install error: {}",
                        task.filename,
                        error.to_string()
                    ));
                }

                successful_mods
                    .lock()
                    .unwrap()
                    .push(task.checksum_md5.clone());
            }
            *status_clone.lock().unwrap() = format!("Finished");
            *is_finished.lock().unwrap() = true;
        })
    }

    fn get_error_list(&self) -> Vec<String> {
        return self.error_list.lock().unwrap().clone();
    }

    fn get_status(&self) -> String {
        return self.current_status.lock().unwrap().clone();
    }

    fn get_successfully_installed_mods(&self) -> Vec<String> {
        return self.successful_mods_md5.lock().unwrap().clone();
    }

    fn is_finished(&self) -> bool {
        return *self.is_finished.lock().unwrap();
    }
}
