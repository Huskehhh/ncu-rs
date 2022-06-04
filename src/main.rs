use clap::{arg, command};
use color_eyre::eyre::Error;
use pbr::ProgressBar;
use serde::Deserialize;
use serde_json::Value;
use tokio::task::JoinHandle;

use std::{collections::HashMap, fs, io::Stdout, time::Instant};

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

#[tokio::main]
async fn main() -> Result<(), Error> {
    let start = Instant::now();

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

    let mut deps: HashMap<String, String> = serde_json::from_value(deps.clone())?;
    let mut dev_deps: HashMap<String, String> = serde_json::from_value(dev_deps.clone())?;

    let dep_count = (deps.len() + dev_deps.len()) as u64;

    let dep_futures = process_dependencies(&deps, false).await;
    let dev_dep_futures = process_dependencies(&dev_deps, true).await;

    let mut updates = vec![];
    let mut pb = ProgressBar::new(dep_count);

    pb.show_speed = false;
    pb.show_time_left = false;

    await_futures(dep_futures, &mut pb, &mut updates).await?;
    await_futures(dev_dep_futures, &mut pb, &mut updates).await?;

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

/// Processes all dependencies in the given map. Returns a Vec containing a JoinHandle to the task
/// for each dependency.
async fn process_dependencies(
    deps: &HashMap<String, String>,
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
                })
            },
        )
        .collect();

    futures
}

/// Gets the latest version of a package via the NPM registry API.
async fn get_package_version(package_name: &str) -> Result<String, Error> {
    let url = format!("{}/{}/latest", API_URL, package_name);

    let resp = reqwest::get(&url)
        .await?
        .json::<GetPackageResponse>()
        .await?;

    Ok(resp.version)
}

/// Inserts new dependencies into the given package_json serde::Value.
pub fn insert_new_maps(
    package_json: &mut Value,
    deps: HashMap<String, String>,
    dev_deps: HashMap<String, String>,
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

        let mut deps: HashMap<String, String> = HashMap::new();
        deps.insert("package-a".to_string(), "^2.0.0".to_string());
        deps.insert("package-b".to_string(), "^3.0.0".to_string());

        let mut dev_deps: HashMap<String, String> = HashMap::new();
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
        let package_version = get_package_version(package).await;
        assert!(package_version.is_ok());
        assert_ne!(package_version.unwrap(), "0.0.0");
    }

    #[tokio::test]
    async fn test_get_package_version_non_existant() {
        let package = "non-existant-package_lol_123123";
        let package_version = get_package_version(package).await;
        assert!(package_version.is_err());
    }
}
