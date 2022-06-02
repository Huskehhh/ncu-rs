use clap::{arg, command};
use color_eyre::eyre::Error;
use pbr::ProgressBar;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

use std::{collections::HashMap, fs, io::Stdout, time::Instant};

const API_URL: &str = "https://registry.npmjs.org/";

#[derive(Debug, Deserialize, Serialize)]
struct PackageJson {
    dependencies: HashMap<String, String>,
    #[serde(rename = "devDependencies")]
    dev_dependencies: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct GetPackageResponse {
    version: String,
}

#[derive(Debug)]
struct PackageUpdateData {
    package_name: String,
    old_version: String,
    new_version: String,
    _dev: bool,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let matches = command!()
        .arg(arg!([path] "Optional path to package.json"))
        .arg(
            arg!(
                -u --update "Enables updating of dep versions in package.json"
            )
            .required(false),
        )
        .get_matches();

    let path = matches.value_of("path").unwrap_or("package.json");
    let _should_update = matches.is_present("update");

    let package_file_contents = fs::read_to_string(&path)?;
    let package_json: PackageJson = serde_json::from_str(&package_file_contents)?;

    let start = Instant::now();

    let deps = package_json.dependencies;
    let dev_deps = package_json.dev_dependencies;

    let dep_count = (deps.len() + dev_deps.len()) as u64;

    let dep_futures = process_dependencies(deps, false).await;
    let dev_dep_futures = process_dependencies(dev_deps, true).await;

    let mut updates = vec![];
    let mut pb = ProgressBar::new(dep_count);

    pb.show_speed = false;
    pb.show_time_left = false;

    await_futures(dep_futures, &mut pb, &mut updates).await?;
    await_futures(dev_dep_futures, &mut pb, &mut updates).await?;

    for update in updates {
        println!(
            "{}     {} => {}",
            update.package_name, update.old_version, update.new_version
        );
    }

    let end = Instant::now();
    println!(
        "Operation completed, duration: {:#.2?}",
        end.duration_since(start)
    );

    Ok(())
}

async fn await_futures(
    futures: Vec<JoinHandle<Option<PackageUpdateData>>>,
    progress_bar: &mut ProgressBar<Stdout>,
    updates_vec: &mut Vec<PackageUpdateData>,
) -> Result<(), Error> {
    for future in futures {
        progress_bar.inc();
        let val = future.await?;
        if let Some(update) = val {
            updates_vec.push(update);
        }
    }
    Ok(())
}

async fn process_dependencies(
    deps: HashMap<String, String>,
    dev: bool,
) -> Vec<tokio::task::JoinHandle<Option<PackageUpdateData>>> {
    let futures: Vec<_> = deps
        .iter()
        .map(
            |(package_name, version)| -> tokio::task::JoinHandle<Option<PackageUpdateData>> {
                let package_name = package_name.clone();
                let version = version.clone();

                tokio::spawn(async move {
                    let cmp_ver = version.replace('^', "").replace('~', "");
                    let ver_prefix = if version.contains('^') {
                        "^"
                    } else if version.contains('~') {
                        "~"
                    } else {
                        ""
                    };

                    match get_package_version(&package_name).await {
                        Ok(latest_version) => {
                            if latest_version != cmp_ver {
                                let package_update_data = PackageUpdateData {
                                    package_name,
                                    old_version: version,
                                    new_version: format!("{}{}", ver_prefix, latest_version),
                                    _dev: dev,
                                };

                                return Some(package_update_data);
                            }
                        }
                        Err(err) => {
                            println!("Error when fetching {package_name} version, {err}");
                        }
                    };

                    None
                })
            },
        )
        .collect();

    futures
}

async fn get_package_version(package_name: &str) -> Result<String, Error> {
    let url = format!("{}/{}/latest", API_URL, package_name);

    let resp = reqwest::get(&url)
        .await?
        .json::<GetPackageResponse>()
        .await?;

    Ok(resp.version)
}
