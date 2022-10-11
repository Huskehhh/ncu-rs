use clap::{arg, command};
use color_eyre::eyre::Error;
use futures::Future;
use indexmap::IndexMap;
use rayon::prelude::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::{
    fs,
    time::{Duration, Instant},
};
use tokio::task::JoinHandle;

use tracing_subscriber::{
    prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, EnvFilter, Registry,
};
use tracing_tree::HierarchicalLayer;

const API_URL: &str = "https://registry.npmjs.org/";
const DEP_KEY: &str = "dependencies";
const DEV_DEP_KEY: &str = "devDependencies";

#[derive(Debug, Deserialize)]
struct GetPackageResponse {
    version: String,
}

#[derive(Debug)]
struct PackageUpdateData {
    package_name: String,
    old_version: String,
    new_version: String,
    dev: bool,
}

fn make_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .expect("Failed to build client")
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let start = Instant::now();

    let client = make_client();

    Registry::default()
        .with(EnvFilter::from_default_env())
        .with(
            HierarchicalLayer::new(2)
                .with_targets(true)
                .with_bracketed_fields(true),
        )
        .init();

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
    let should_update = matches.is_present("update");

    let package_file_contents = fs::read_to_string(&path)?;
    let mut package_json: serde_json::Value = serde_json::from_str(&package_file_contents)?;

    let deps = package_json.get(DEP_KEY).unwrap();
    let dev_deps = package_json.get(DEV_DEP_KEY).unwrap();

    let mut deps: IndexMap<String, String> = serde_json::from_value(deps.clone())?;
    let mut dev_deps: IndexMap<String, String> = serde_json::from_value(dev_deps.clone())?;

    let dep_futures = process_dependencies(&client, &deps, false);
    let dev_dep_futures = process_dependencies(&client, &dev_deps, true);

    let mut did_update_packages = false;
    for update in updates {
        did_update_packages = true;
        println!(
            "{}     {} => {}",
            update.package_name, update.old_version, update.new_version
        );

        // If we should update the package.json file, update the relevant map.
        if should_update {
            if update.dev {
                dev_deps.insert(update.package_name, update.new_version);
            } else {
                deps.insert(update.package_name, update.new_version);
            }
        }
    }

    // Finally, merge the newly updated versions into the previous value struct.
    if should_update {
        insert_new_maps(&mut package_json, deps, dev_deps)?;

        // Write the updated package.json file.
        let package_file_contents = serde_json::to_string_pretty(&package_json)?;
        fs::write(&path, package_file_contents)?;

        if did_update_packages {
            println!(
                "Updated {}. Please install the updated packages. (npm/yarn/pnpm install)!",
                path
            );
        } else {
            println!("No dependency updates found.");
        }
    }

    let end = Instant::now();
    println!(
        "Operation completed, duration: {:#.2?}",
        end.duration_since(start)
    );

    Ok(())
}

/// Helper function to await all dep futures and update the progress bar according to progress.
async fn await_futures(
    futures: Vec<impl Future<Output = Option<PackageUpdateData>>>,
    updates_vec: &mut Vec<PackageUpdateData>,
) -> Result<(), Error> {
    for future in futures {
        let val = future.await;
        if let Some(update) = val {
            updates_vec.push(update);
        }
    }
    Ok(())
}

/// Processes all dependencies in the given map. Returns a Vec containing a JoinHandle to the task
/// for each dependency.
async fn process_dependencies(
    client: &Client,
    deps: &IndexMap<String, String>,
    dev: bool,
) -> Vec<PackageUpdateData> {
    let mut updates = vec![];
    let mut update_dest = vec![];

    deps.par_iter().for_each(|(package_name, version)| {
        let update = compare_package_version(client, version.clone(), package_name.clone(), dev);
    });

    await_futures(updates, &mut update_dest).await;

    update_dest
}

async fn compare_package_version(
    client: &Client,
    version: String,
    package_name: String,
    dev: bool,
) -> Option<PackageUpdateData> {
    let cmp_ver = version.replace('^', "").replace('~', "");
    let ver_prefix = if version.contains('^') {
        "^"
    } else if version.contains('~') {
        "~"
    } else {
        ""
    };

    match get_package_version(client, &package_name).await {
        Ok(latest_version) => {
            if latest_version != cmp_ver {
                let package_update_data = PackageUpdateData {
                    package_name,
                    old_version: version,
                    new_version: format!("{}{}", ver_prefix, latest_version),
                    dev,
                };

                return Some(package_update_data);
            }
        }
        Err(err) => {
            println!("Error when fetching {package_name} version, {err}");
        }
    };

    None
}

/// Gets the latest version of a package via the NPM registry API.
async fn get_package_version(client: &Client, package_name: &str) -> Result<String, Error> {
    let url = format!("{}/{}/latest", API_URL, package_name);

    let resp = client
        .get(&url)
        .send()
        .await?
        .json::<GetPackageResponse>()
        .await?;

    Ok(resp.version)
}

/// Inserts new dependencies into the given package_json serde::Value.
pub fn insert_new_maps(
    package_json: &mut Value,
    deps: IndexMap<String, String>,
    dev_deps: IndexMap<String, String>,
) -> Result<(), Error> {
    if let Some(deps_value) = package_json.get_mut(DEP_KEY) {
        *deps_value = serde_json::to_value(deps)?;
    }
    if let Some(dev_deps_value) = package_json.get_mut(DEV_DEP_KEY) {
        *dev_deps_value = serde_json::to_value(dev_deps)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_insert_new_maps() {
        let mut package_json = json!({
            "name": "abc123",
            "dependencies": {
                "package-a": "^1.0.0",
                "package-b": "^2.0.0",
            },
            "devDependencies": {
                "package-c": "^3.0.0",
                "package-d": "^4.0.0",
            }
        });

        let mut deps: IndexMap<String, String> = IndexMap::new();
        deps.insert("package-a".to_string(), "^2.0.0".to_string());
        deps.insert("package-b".to_string(), "^3.0.0".to_string());

        let mut dev_deps: IndexMap<String, String> = IndexMap::new();
        dev_deps.insert("package-c".to_string(), "^3.5.0".to_string());
        dev_deps.insert("package-d".to_string(), "^4.0.0".to_string());

        // Expect the new maps to be inserted into the package.json file.
        let result = insert_new_maps(&mut package_json, deps, dev_deps);
        assert!(result.is_ok());
        assert_eq!(
            package_json,
            json!({
                "name": "abc123",
                "dependencies": {
                    "package-a": "^2.0.0",
                    "package-b": "^3.0.0",
                },
                "devDependencies": {
                    "package-c": "^3.5.0",
                    "package-d": "^4.0.0",
                }
            })
        );
    }

    #[tokio::test]
    async fn test_get_package_version() {
        let package = "react";
        let package_version = get_package_version(&client, package).await;
        assert!(package_version.is_ok());
        assert_ne!(package_version.unwrap(), "0.0.0");
    }

    #[tokio::test]
    async fn test_get_package_version_non_existant() {
        let client = make_client();
        let package = "non-existant-package_lol_123123";
        let package_version = get_package_version(&client, package).await;
        assert!(package_version.is_err());
    }

    #[tokio::test]
    async fn test_process_dependencies_and_await_futures() {
        let client = make_client();

        let mut deps: IndexMap<String, String> = IndexMap::new();
        deps.insert("react".to_string(), "^2.0.0".to_string());
        deps.insert("recoil".to_string(), "~3.0.0".to_string());

        let futures = process_dependencies(&client, &deps, false);
        assert_eq!(futures.len(), 2);

        let mut updates_vec: Vec<PackageUpdateData> = vec![];

        let futures = await_futures(futures, &mut updates_vec).await;
        assert!(futures.is_ok());

        for update in updates_vec {
            if update.package_name == "react" {
                assert_eq!(update.old_version, "^2.0.0");
                assert_ne!(update.old_version, update.new_version);
            } else if update.package_name == "recoil" {
                assert_eq!(update.old_version, "~3.0.0");
                assert_ne!(update.old_version, update.new_version);
            } else {
                panic!("Unexpected package name: {}", update.package_name);
            }
        }
    }
}
