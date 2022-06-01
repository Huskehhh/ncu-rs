use color_eyre::eyre::Error;
use pbr::ProgressBar;
use serde::Deserialize;

use std::{collections::HashMap, fs, time::Instant};

const API_URL: &str = "https://registry.npmjs.org/";

#[derive(Debug, Deserialize)]
struct PackageJson {
    dependencies: HashMap<String, String>,
    #[serde(rename = "devDependencies")]
    dev_dependencies: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct GetPackageResponse {
    version: String,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let package_file_contents = fs::read_to_string("package.json")?;
    let package_json: PackageJson = serde_json::from_str(&package_file_contents)?;

    let start = Instant::now();

    let deps = package_json.dependencies;
    let dev_deps = package_json.dev_dependencies;

    let dep_count = (deps.len() + dev_deps.len()) as u64;

    let dep_futures = process_dependencies(deps).await;
    let dev_dep_futures = process_dependencies(dev_deps).await;

    let mut updates = vec![];

    let mut pb = ProgressBar::new(dep_count);
    pb.show_speed = false;
    pb.show_time_left = false;

    for future in dep_futures {
        pb.inc();
        let val = future.await?;
        if let Some(update) = val {
            updates.push(update);
        }
    }

    for future in dev_dep_futures {
        pb.inc();
        let val = future.await?;
        if let Some(update) = val {
            updates.push(update);
        }
    }

    updates.iter().for_each(|update| {
        println!("{}", update);
    });

    let end = Instant::now();
    println!(
        "Operation completed, duration: {:#.2?}",
        end.duration_since(start)
    );

    Ok(())
}

async fn process_dependencies(
    deps: HashMap<String, String>,
) -> Vec<tokio::task::JoinHandle<Option<String>>> {
    let futures: Vec<_> = deps
        .iter()
        .map(
            |(package_name, version)| -> tokio::task::JoinHandle<Option<String>> {
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
                                return Some(format!(
                                    "{package_name} {version} => {ver_prefix}{latest_version}"
                                ));
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
