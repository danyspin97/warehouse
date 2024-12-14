// #![feature(slice_concat_trait)]

use std::{fmt, fs::File, io::Write, path::Path, process::Stdio};

use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand};
use eyre::{bail, Context, Result};

#[derive(serde::Deserialize, Debug)]
pub struct PackageOption {
    pub name: String,
    pub description: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct Dependency {
    pub name: String,
    pub version: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct Dependencies(Vec<Dependency>);

#[derive(Debug)]
pub struct Package {
    pub name: String,
    pub summary: String,
    pub description: String,
    pub homepage: String,
    pub licenses: String,
    pub options: Vec<PackageOption>,
    pub dependencies: Dependencies,
    pub patches: Vec<String>,
    pub sources: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
enum DependencyType {
    Build,
    BuildRun,
    Run,
    Test,
}

#[derive(Debug, serde::Deserialize)]
enum ChildType {
    Sources,
    Dependencies,
    Module,
}

impl Package {
    pub fn new(json: serde_json::Value) -> Result<Self> {
        let json = json["children"].as_array().unwrap().first().unwrap();
        let name = json["args"].as_array().unwrap()[0]
            .as_str()
            .unwrap()
            .to_string();
        let attrs = &json["attrs"];
        let mut sources: Vec<String> = Vec::new();
        let mut dependencies = Dependencies(Vec::new());
        let mut parse_child = |child: &serde_json::Value| -> Result<()> {
            match serde_json::from_value(child["type"].clone())? {
                // TODO: Also consider children
                ChildType::Sources => sources.extend(
                    child["attrs"]
                        .as_object()
                        .unwrap()
                        .values()
                        .map(|v| v.as_str().unwrap().to_string()),
                ),
                ChildType::Dependencies => {
                    let new_deps: Vec<Dependency> = serde_json::from_value(child["attrs"].clone())?;
                    match child["args"]
                        .as_array()
                        .unwrap()
                        .first()
                        .unwrap()
                        .as_str()
                        .unwrap()
                    {
                        "build" => dependencies.build = new_deps,
                        "build+run" => dependencies.build_run = new_deps,
                        "run" => dependencies.run = new_deps,
                        "test" => dependencies.test = new_deps,
                        _ => bail!("Unknown dependency type"),
                    }
                }
                ChildType::Module => {}
            }

            Ok(())
        };

        for child in json.get("children").unwrap().as_array().unwrap() {
            parse_child(child)?;
        }
        Ok(Self {
            name,
            summary: attrs["summary"].to_string(),
            description: attrs
                .get("description")
                .map(|s| s.to_string())
                .unwrap_or_default(),
            homepage: attrs["homepage"].to_string(),
            licenses: attrs["licenses"].to_string(),
            options: Vec::new(),
            dependencies: Dependencies::new(),
            patches: attrs["patches"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .map(|s| s.as_str().unwrap().to_string())
                .collect(),
            sources,
        })
    }
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Opts {
    #[clap(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Build(BuildCmd),
    Install(InstallCmd),
}

#[derive(Args)]
struct BuildCmd {
    #[clap(long)]
    print_oil_metadata: bool,
    #[clap(long, conflicts_with = "print_oil_metadata")]
    print_package_metadata: bool,
    #[clap(short, long, help = "Build the package locally")]
    local: bool,
    #[clap(short, long, help = "Root of the warehouse")]
    buildroot: Option<Utf8PathBuf>,
    pkg_name: String,
    pkg_version: String,
}

#[derive(Args)]
struct InstallCmd {}

fn main() {
    let args = Opts::parse();

    let res = match args.cmd {
        Cmd::Build(build_args) => build(build_args),
        Cmd::Install(_) => install(),
    };

    if let Err(err) = res {
        eprintln!("{:?}", err);
        std::process::exit(1);
    }
}

fn install() -> std::result::Result<(), eyre::Error> {
    todo!()
}

fn build(args: BuildCmd) -> Result<()> {
    let json = get_metadata(&args)?;

    if args.print_oil_metadata {
        println!("{}", serde_json::to_string_pretty(&json).unwrap());
        return Ok(());
    }

    let package = Package::new(json)?;

    if args.print_package_metadata {
        println!("{package:?}");
        return Ok(());
    }

    let builddir = "/tmp/warehouse";
    std::fs::create_dir_all(Path::new(builddir).join("files")).unwrap();
    for p in &package.patches {
        std::fs::copy(
            Path::new("packages")
                .join(&package.name)
                .join("patches")
                .join(&p),
            Path::new(builddir).join("files").join(&p),
        )
        .unwrap();
    }
    let builddir = "/tmp/warehouse/build";
    let destdir = "/tmp/warehouse/buildroot";
    std::fs::create_dir_all(builddir).unwrap();
    std::fs::create_dir_all(destdir).unwrap();
    std::fs::create_dir_all(Path::new(builddir).join("files")).unwrap();
    let sources = print_oil_array(&package.sources);
    println!("{sources}");
    let patches = print_oil_array(&package.patches);
    let prefix = "/usr";
    let bindir = "/usr";
    let nproc = num_cpus::get();
    let cc = "sccache gcc";
    let generated = format!(
        "
const name = '{}'
const version = '{}'
const basedir = '{builddir}'
const builddir = '{builddir}'
const sources =[
	{sources}
]
const patches =[
    {patches}
]
const prefix = '{prefix}'
const bindir = '{bindir}'
const nproc = {nproc}
const CC = '{cc}'
const DESTDIR = '{destdir}'

export CC DESTDIR
",
        package.name, &args.pkg_version
    );
    let mut file = File::create(Path::new(builddir).join("config.oil")).unwrap();
    file.write_all(generated.as_bytes()).unwrap();
    std::process::Command::new("/home/danyspin97/proj/coding/oil-0.14.0/_bin/oil.ovm")
        .args(&["oil", "build.oil", &package.name])
        .spawn()
        .unwrap()
        .wait()
        .unwrap();

    // Setup the config and variables

    Ok(())
}

fn get_metadata(args: &BuildCmd) -> Result<serde_json::Value> {
    let output = String::from_utf8(
        std::process::Command::new("ysh")
            .args(["metadata.oil", &args.pkg_name, &args.pkg_version])
            .stdout(Stdio::piped())
            .spawn()?
            .wait_with_output()?
            .stdout,
    )
    .unwrap();
    serde_json::from_str(&output).context("Failed to parse metadata")
}

fn print_oil_array(elements: &[String]) -> String {
    elements
        .iter()
        .map(|e| format!("'{e}'"))
        .collect::<Vec<String>>()
        .join(",\n\t\t")
}
