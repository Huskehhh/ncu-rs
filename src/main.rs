use clap::{arg, command};
use color_eyre::eyre::Error;
use indexmap::IndexMap;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::{
    fs,
    time::{Duration, Instant},
};
use tokio::sync::OnceCell;

use tracing_subscriber::{
    prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, EnvFilter, Registry,
};
use tracing_tree::HierarchicalLayer;

static CLIENT: OnceCell<Client> = OnceCell::const_new();

const API_URL: &str = "https://registry.npmjs.org";
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
}

async fn make_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap()
}

async fn get_client() -> &'static Client {
    CLIENT.get_or_init(make_client).await
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let start = Instant::now();

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

    let dep_updates = process_dependencies(&deps).await;
    let dev_dep_updates = process_dependencies(&dev_deps).await;

    let did_update_pkgs = build_updates(dep_updates, should_update, &mut deps).await;
    let did_update_dev_pkgs = build_updates(dev_dep_updates, should_update, &mut dev_deps).await;

    // Finally, merge the newly updated versions into the previous value struct.
    if should_update {
        insert_new_maps(&mut package_json, deps, dev_deps)?;

        // Write the updated package.json file.
        let package_file_contents = serde_json::to_string_pretty(&package_json)?;
        fs::write(&path, package_file_contents)?;

        if did_update_pkgs || did_update_dev_pkgs {
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

/// Processes all dependencies in the given map. Returns a Vec containing a JoinHandle to the task
/// for each dependency.
async fn process_dependencies(deps: &IndexMap<String, String>) -> Vec<Option<PackageUpdateData>> {
    let mut updates = vec![];

    for (package_name, version) in deps {
        let update = compare_package_version(version.clone(), package_name.clone()).await;
        updates.push(update);
    }

    updates
}

async fn build_updates(
    updates: Vec<Option<PackageUpdateData>>,
    should_update: bool,
    dest: &mut IndexMap<String, String>,
) -> bool {
    let mut did_update_packages = false;

    updates.into_iter().for_each(|update| {
        if let Some(update) = update {
            println!(
                "{}     {} => {}",
                update.package_name, update.old_version, update.new_version
            );

            // If we should update the package.json file, update the relevant map.
            if should_update {
                dest.insert(update.package_name, update.new_version);
            }

            did_update_packages = true;
        }
    });

    did_update_packages
}

async fn compare_package_version(
    version: String,
    package_name: String,
) -> Option<PackageUpdateData> {
    let client = get_client().await;
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
        let client = make_client().await;
        let package = "react";
        let package_version = get_package_version(&client, package).await;
        assert!(package_version.is_ok());
        assert_ne!(package_version.unwrap(), "0.0.0");
    }

    #[tokio::test]
    async fn test_get_package_version_non_existant() {
        let client = make_client().await;
        let package = "non-existant-package_lol_123123";
        let package_version = get_package_version(&client, package).await;
        assert!(package_version.is_err());
    }

    #[tokio::test]
    async fn test_process_dependencies_and_await_futures() {
        let mut deps: IndexMap<String, String> = IndexMap::new();
        deps.insert("react".to_string(), "^2.0.0".to_string());
        deps.insert("recoil".to_string(), "~3.0.0".to_string());

        let updates = process_dependencies(&deps).await;
        assert_eq!(updates.len(), 2);

        updates.into_iter().for_each(|update| {
            if let Some(update) = update {
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
        });
    }
}
