//! `--export`: convert and export documents from the command line, with no
//! window and no user data written. Any file the app opens (.mvec, SVG,
//! PDF, AI, EPS) can be exported to any export format; several inputs make
//! a batch. `--slices` writes every layer and artboard's export settings.

use std::path::{Path, PathBuf};

use crate::io::export::{self, Area, Format, Options};
use crate::print::ColorOutput;

pub const USAGE: &str = "\
usage: mtzd-vector --export [OPTIONS] INPUT...

Exports each INPUT (.mvec, .svg, .pdf, .ai, .eps) without opening a window.

Output
  -o, --output PATH     file to write (one input), or a folder (several inputs);
                        default: next to the input, named after it
  -f, --format FMT      png, jpg, webp, avif, svg, pdf, eps, dxf
                        (default: from the output's extension, else png)
  --artboard NAME|N     export that artboard (name, or number from 1)
  --all-artboards       every artboard: one file each, or pages of one PDF
  --slices              write the export settings of every layer and artboard
                        (Export section of the Design panel) into the output folder

Raster
  -s, --scale N         pixels per document pixel (default 1)
  --transparent         no artboard background (PNG, WebP, AVIF)
  -q, --quality N       JPG / AVIF quality 1-100 (default 90); makes WebP lossy

Vector
  --outline-text        text as outlines (SVG, PDF)
  --precision N         SVG decimal places (default 3)
  --minify              SVG on one line, no indentation
  --responsive          SVG scales to its container (no width/height)
  --no-ids              SVG without layer-name ids

Print (PDF)
  --cmyk [PROFILE.icc]  CMYK output (best installed profile if none given)
  --pdfx                PDF/X-4 (implies --cmyk)
  --bleed MM            bleed around each page
  --crop-marks          trim marks in the bleed

  --quiet               print nothing but errors
  -h, --help            this help
";

#[derive(Debug, Default)]
struct Args {
    inputs: Vec<PathBuf>,
    output: Option<PathBuf>,
    format: Option<Format>,
    artboard: Option<String>,
    all: bool,
    slices: bool,
    opts: Options,
    cmyk: Option<Option<PathBuf>>,
    quiet: bool,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args { opts: Options { scale: 1.0, ..Default::default() }, ..Default::default() };
    let mut i = 0;
    let value = |i: &mut usize, name: &str| -> Result<String, String> {
        *i += 1;
        args.get(*i).cloned().ok_or(format!("{name} needs a value"))
    };
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-h" | "--help" => return Err(String::new()),
            "-o" | "--output" => a.output = Some(value(&mut i, arg)?.into()),
            "-f" | "--format" => {
                let v = value(&mut i, arg)?;
                a.format = Some(Format::from_ext(&v).ok_or(format!("unknown format {v}"))?);
            }
            "--artboard" => a.artboard = Some(value(&mut i, arg)?),
            "--all-artboards" => a.all = true,
            "--slices" => a.slices = true,
            "-s" | "--scale" => a.opts.scale = value(&mut i, arg)?.trim_end_matches(['x', 'X']).parse::<f64>().ok().filter(|s| *s > 0.0 && *s <= 64.0).ok_or("--scale takes a number from 0 to 64")?,
            "--transparent" => a.opts.background = false,
            "-q" | "--quality" => {
                a.opts.quality = value(&mut i, arg)?.parse::<u8>().ok().filter(|q| (1..=100).contains(q)).ok_or("--quality takes 1 to 100")?;
                a.opts.webp_lossy = true;
            }
            "--outline-text" => a.opts.text_outlines = true,
            "--precision" => a.opts.svg.precision = value(&mut i, arg)?.parse::<usize>().ok().filter(|p| *p <= 8).ok_or("--precision takes 0 to 8")?,
            "--minify" => a.opts.svg.minify = true,
            "--responsive" => a.opts.svg.responsive = true,
            "--no-ids" => a.opts.svg.ids = false,
            "--cmyk" => {
                // An optional profile path follows.
                let next = args.get(i + 1).filter(|n| {
                    let l = n.to_ascii_lowercase();
                    l.ends_with(".icc") || l.ends_with(".icm")
                });
                a.cmyk = Some(next.map(PathBuf::from));
                if next.is_some() {
                    i += 1;
                }
            }
            "--pdfx" => {
                a.opts.print.pdfx = true;
                a.cmyk.get_or_insert(None);
            }
            "--bleed" => a.opts.print.bleed_mm = value(&mut i, arg)?.parse::<f64>().ok().filter(|b| (0.0..=50.0).contains(b)).ok_or("--bleed takes 0 to 50 (mm)")?,
            "--crop-marks" => a.opts.print.crop_marks = true,
            "--quiet" => a.quiet = true,
            s if s.starts_with('-') && s.len() > 1 => return Err(format!("unknown option {s}")),
            s => a.inputs.push(s.into()),
        }
        i += 1;
    }
    if a.inputs.is_empty() {
        return Err("no input files".into());
    }
    if let Some(cmyk) = &a.cmyk {
        a.opts.print.color = ColorOutput::Cmyk;
        a.opts.print.profile = match cmyk {
            Some(p) => Some(p.clone()),
            None => crate::print::cmyk_profiles().first().map(|p| p.path.clone()),
        };
        if a.opts.print.pdfx && a.opts.print.profile.is_none() {
            return Err("--pdfx needs a CMYK profile (none installed: pass --cmyk PROFILE.icc)".into());
        }
    }
    Ok(a)
}

/// Run `--export ARGS…`; returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    crate::recent::ENABLED.store(false, std::sync::atomic::Ordering::Relaxed);
    let a = match parse(args) {
        Ok(a) => a,
        Err(e) if e.is_empty() => {
            print!("{USAGE}");
            return 0;
        }
        Err(e) => {
            eprintln!("mtzd-vector --export: {e}\n\n{USAGE}");
            return 2;
        }
    };
    let mut fonts = crate::fonts::FontManager::new(egui::Context::default());
    let mut failed = 0;
    for input in &a.inputs {
        match export_one(&a, input, &mut fonts) {
            Ok(files) => {
                if !a.quiet {
                    for f in files {
                        println!("{}", f.display());
                    }
                }
            }
            Err(e) => {
                eprintln!("{}: {e}", input.display());
                failed += 1;
            }
        }
    }
    if failed > 0 { 1 } else { 0 }
}

fn export_one(a: &Args, input: &Path, fonts: &mut crate::fonts::FontManager) -> Result<Vec<PathBuf>, String> {
    let mut doc = crate::io::open(input, fonts)?;
    crate::text_flow::resolve_flows_uncached(&mut doc, fonts);
    let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "export".into());
    let batch = a.inputs.len() > 1;
    if a.slices {
        let dir = a.output.clone().unwrap_or_else(|| input.with_extension("").with_file_name(format!("{stem} export")));
        let dir = if batch { dir.join(&stem) } else { dir };
        let targets = export::slice_targets(&doc);
        if targets.is_empty() {
            return Err("no layers or artboards have export settings".into());
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        return export::export_slices(&doc, &targets, &dir, fonts);
    }
    let out_is_dir = a.output.as_ref().is_some_and(|o| batch || o.is_dir() || o.as_os_str().to_string_lossy().ends_with('/'));
    let format = a
        .format
        .or_else(|| a.output.as_ref().filter(|_| !out_is_dir).and_then(|o| o.extension()).and_then(|e| Format::from_ext(&e.to_string_lossy())))
        .unwrap_or(Format::Png);
    let path = match &a.output {
        Some(o) if out_is_dir => {
            std::fs::create_dir_all(o).map_err(|e| format!("{}: {e}", o.display()))?;
            o.join(format!("{stem}.{}", format.ext()))
        }
        Some(o) => o.clone(),
        None => input.with_extension(format.ext()),
    };
    if path == input {
        return Err("the output would overwrite the input".into());
    }
    let active = match &a.artboard {
        None => doc.primary().id,
        Some(name) => {
            let by_name = doc.artboards.iter().find(|b| b.name.eq_ignore_ascii_case(name));
            let by_num = name.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|n| doc.artboards.get(n));
            by_name.or(by_num).map(|b| b.id).ok_or_else(|| {
                let names: Vec<&str> = doc.artboards.iter().map(|b| b.name.as_str()).collect();
                format!("no artboard {name} (has: {})", names.join(", "))
            })?
        }
    };
    let opts = Options { format, area: if a.all { Area::AllArtboards } else { Area::Artboard }, ..a.opts.clone() };
    export::export(&doc, &[], active, &opts, &path, fonts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parses_options() {
        let a = parse(&args("in.mvec -o out.webp -s 2x --transparent --precision 2 --minify --bleed 3 --crop-marks")).unwrap();
        assert_eq!(a.inputs, vec![PathBuf::from("in.mvec")]);
        assert_eq!(a.opts.scale, 2.0);
        assert!(!a.opts.background && a.opts.svg.minify && a.opts.print.crop_marks);
        assert_eq!(a.opts.svg.precision, 2);
        assert!(parse(&args("--scale 0 in.mvec")).is_err());
        assert!(parse(&args("--bogus in.mvec")).is_err());
        assert!(parse(&args("-o x.png")).is_err());
        assert_eq!(parse(&args("--help")).unwrap_err(), "");
        let a = parse(&args("a.mvec --cmyk /x/y.icc b.svg")).unwrap();
        assert_eq!(a.opts.print.profile, Some(PathBuf::from("/x/y.icc")));
        assert_eq!(a.inputs.len(), 2);
    }

    #[test]
    fn exports_batch_and_artboards() {
        let dir = std::env::temp_dir().join(format!("mtzd-cli-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut d = crate::document::Document::default();
        d.artboards[0].w = 40.0;
        d.artboards[0].h = 20.0;
        d.artboards[0].name = "Cover".into();
        let mut b = d.artboards[0].clone();
        b.id = 999;
        b.name = "Back".into();
        b.x += 100.0;
        b.w = 10.0;
        d.artboards.push(b);
        let input = dir.join("doc.mvec");
        crate::io::save_native(&d, &input).unwrap();
        let svg_in = dir.join("pic.svg");
        std::fs::write(&svg_in, r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#f00"/></svg>"##).unwrap();
        let s = |p: &Path| p.to_string_lossy().into_owned();
        // One file, format from the extension, chosen artboard and scale.
        let out = dir.join("back.png");
        assert_eq!(run(&[s(&input), "-o".into(), s(&out), "--artboard".into(), "back".into(), "-s".into(), "3".into(), "--quiet".into()]), 0);
        let img = image::open(&out).unwrap();
        assert_eq!((img.width(), img.height()), (30, 60));
        // A batch into a folder.
        let outdir = dir.join("out");
        assert_eq!(run(&[s(&input), s(&svg_in), "-o".into(), s(&outdir), "-f".into(), "pdf".into(), "--quiet".into()]), 0);
        assert!(outdir.join("doc.pdf").exists() && outdir.join("pic.pdf").exists());
        // Every artboard: one PNG each.
        assert_eq!(run(&[s(&input), "--all-artboards".into(), "-o".into(), s(&dir.join("all.png")), "--quiet".into()]), 0);
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().filter(|n| n.starts_with("all") && n.ends_with(".png")).count() == 2, "{names:?}");
        // Errors: a missing artboard, a missing file.
        assert_eq!(run(&[s(&input), "--artboard".into(), "Nope".into(), "--quiet".into()]), 1);
        assert_eq!(run(&[s(&dir.join("missing.mvec")), "--quiet".into()]), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
