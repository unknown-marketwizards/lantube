use std::{
    env, fs,
    io::{self, Write},
    path::Path,
    process::Command,
};

const FRONTEND: &str = "frontend";
const INPUTS: &[&str] = &[
    "package.json",
    "package-lock.json",
    "index.html",
    "tsconfig.json",
    "vite.config.ts",
    "src",
    "public",
];
const DIST: &str = "frontend-dist";

fn main() -> io::Result<()> {
    println!("cargo::rerun-if-env-changed=PATH");
    let frontend = Path::new(FRONTEND);
    let out_dir = env::var_os("OUT_DIR").expect("Cargo must set OUT_DIR");
    let out_dir = Path::new(&out_dir);
    let project = tempfile::tempdir_in(out_dir)?;

    // 把依赖和构建中间文件留在 Cargo 输出目录，方便前后端并行开发。
    for input in INPUTS {
        let source = frontend.join(input);
        println!("cargo::rerun-if-changed={}", source.display());
        copy_input(&source, &project.path().join(input))?;
    }

    run_npm(
        project.path(),
        &["ci", "--include=dev", "--no-audit", "--no-fund"],
    )?;
    run_npm(
        project.path(),
        &[
            "run",
            "build",
            "--",
            "--outDir",
            "../frontend-dist",
            "--emptyOutDir",
        ],
    )?;

    let dist = out_dir.join(DIST);
    let mut assets = Vec::new();
    collect_assets(&dist, &dist, &mut assets)?;
    assets.sort_unstable();
    assert!(
        dist.join("index.html").is_file(),
        "Vite did not generate index.html"
    );

    let mut manifest = fs::File::create(out_dir.join("frontend_assets.rs"))?;
    writeln!(
        manifest,
        "fn embedded_asset(path: &str) -> Option<&'static [u8]> {{"
    )?;
    writeln!(manifest, "    match path {{")?;
    for asset in assets {
        writeln!(
            manifest,
            "        {asset:?} => Some(include_bytes!({:?})),",
            format!("{DIST}/{asset}"),
        )?;
    }
    writeln!(manifest, "        _ => None,\n    }}\n}}")?;
    Ok(())
}

fn copy_input(source: &Path, destination: &Path) -> io::Result<()> {
    if source.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_input(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        fs::copy(source, destination)?;
    }
    Ok(())
}

fn run_npm(directory: &Path, arguments: &[&str]) -> io::Result<()> {
    let program = if cfg!(windows) { "npm.cmd" } else { "npm" };
    let status = Command::new(program)
        .current_dir(directory)
        .args(arguments)
        .status()
        .map_err(|error| io::Error::new(
            error.kind(),
            format!("Could not start {program}: {error}. Install Node.js and npm before building LanTube."),
        ))?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "npm {} failed with {status}",
            arguments.join(" ")
        )));
    }
    Ok(())
}

fn collect_assets(root: &Path, directory: &Path, assets: &mut Vec<String>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_assets(root, &path, assets)?;
        } else {
            let relative = path
                .strip_prefix(root)
                .expect("asset must be under Vite output");
            let relative = relative.to_str().expect("asset path must be UTF-8");
            assets.push(relative.replace(std::path::MAIN_SEPARATOR, "/"));
        }
    }
    Ok(())
}
