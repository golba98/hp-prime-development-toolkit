//! File-based assertions; execution is supplied by the supervised CLI runner.
use crate::{
    Report, Request,
    execution::{atomic_bytes, protect_output, save_image},
};
use ppl_compat::differential::{Image, compare_images};
use ppl_graphics::Grob;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    #[serde(rename = "test")]
    pub tests: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub name: String,
    pub file: PathBuf,
    #[serde(default = "default_operation")]
    pub operation: String,
    pub entry: Option<String>,
    #[serde(default = "default_args")]
    pub args: String,
    #[serde(default)]
    pub keys: Option<String>,
    #[serde(default, rename = "input")]
    pub inputs: Vec<String>,
    #[serde(default, rename = "choose")]
    pub choices: Vec<usize>,
    pub seed: Option<u64>,
    pub max_time: Option<f64>,
    pub max_steps: Option<u64>,
    #[serde(default)]
    pub expect: Expected,
}
fn default_operation() -> String {
    "run".into()
}
fn default_args() -> String {
    "[]".into()
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Expected {
    pub status: Option<String>,
    pub stdout: Option<Vec<String>>,
    pub return_value: Option<Value>,
    pub diagnostic_codes: Option<Vec<String>>,
    pub png: Option<PathBuf>,
    pub channel_tolerance: u8,
    pub max_changed_pixels: usize,
}

pub fn run_suite(
    path: &Path,
    artifacts: &Path,
    update: bool,
    mut run: impl FnMut(Request) -> Report,
) -> Report {
    let started = Instant::now();
    let mut report = Report::new("test", Some(path.to_owned()));
    let text = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            report.fail("io_error", 5, e.to_string());
            return report;
        }
    };
    let manifest: Manifest = match toml::from_str(&text) {
        Ok(m) => m,
        Err(e) => {
            report.fail("invalid_request", 2, e.to_string());
            return report;
        }
    };
    if manifest.schema_version != 1 || manifest.tests.is_empty() {
        report.fail(
            "invalid_request",
            2,
            "expected schema_version=1 and at least one [[test]]",
        );
        return report;
    }
    let root = path.parent().unwrap_or(Path::new("."));
    let mut results = vec![];
    let mut passed = 0;
    let mut failed = 0;
    let protected: Vec<_> = std::iter::once(path.to_owned())
        .chain(manifest.tests.iter().map(|c| root.join(&c.file)))
        .collect();
    let references: Vec<_> = manifest
        .tests
        .iter()
        .filter_map(|c| c.expect.png.as_ref().map(|p| root.join(p)))
        .collect();
    for (index, case) in manifest.tests.into_iter().enumerate() {
        let mut request = Request {
            operation: case.operation.clone(),
            file: root.join(&case.file),
            entry: case.entry,
            args: case.args,
            keys: case.keys,
            inputs: case.inputs,
            choices: case.choices,
            ..Default::default()
        };
        if !matches!(case.operation.as_str(), "run" | "check") {
            report.fail("invalid_request", 2, "test operation must be run or check");
            return report;
        }
        if let Some(s) = case.seed {
            request.seed = s;
        }
        if let Some(s) = case.max_time {
            request.max_time = Some(s);
        }
        if let Some(s) = case.max_steps {
            request.max_steps = s;
        }
        if request.max_time.is_some_and(|s| !s.is_finite() || s <= 0.) {
            report.fail(
                "invalid_request",
                2,
                "test max_time must be positive and finite",
            );
            return report;
        }
        let temp = match tempfile::tempdir() {
            Ok(t) => t,
            Err(e) => {
                report.fail("io_error", 5, e.to_string());
                return report;
            }
        };
        if case.expect.png.is_some() {
            request.png = Some(temp.path().join("screen.png"));
        }
        let mut actual = run(request);
        if actual.status == "cancelled" {
            report.fail("cancelled", 130, "test run cancelled");
            break;
        }
        let mut errors = vec![];
        let expected_status = case.expect.status.as_deref().unwrap_or("success");
        if actual.status != expected_status {
            errors.push(format!(
                "status: expected {expected_status}, got {} ({})",
                actual.status,
                actual.error.as_deref().unwrap_or("")
            ));
        }
        if case
            .expect
            .stdout
            .as_ref()
            .is_some_and(|s| s != &actual.output)
        {
            errors.push("PRINT output differs".into());
        }
        if case
            .expect
            .return_value
            .as_ref()
            .is_some_and(|v| Some(v) != actual.return_value.as_ref())
        {
            errors.push("return value differs".into());
        }
        if let Some(expected) = &case.expect.diagnostic_codes {
            let observed: Vec<_> = actual.diagnostics.iter().map(|d| d.code.clone()).collect();
            if expected != &observed {
                errors.push(format!(
                    "diagnostics differ: expected {expected:?}, got {observed:?}"
                ));
            }
        }
        let mut image_difference = Value::Null;
        if let Some(reference) = &case.expect.png {
            let reference = root.join(reference);
            if let Err(e) = protect_output(&reference, &protected) {
                report.fail("invalid_request", 2, e);
                return report;
            }
            let actual_path = temp.path().join("screen.png");
            let comparison = (|| -> Result<(), String> {
                let bytes = std::fs::read(&actual_path)
                    .map_err(|e| format!("no execution screenshot: {e}"))?;
                if update {
                    if let Some(parent) = reference.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    atomic_bytes(&reference, &bytes).map_err(|e| e.to_string())?;
                    report.artifacts.push(reference.clone());
                    return Ok(());
                }
                let a = Grob::read_png(&bytes)?;
                let b = Grob::read_png(
                    &std::fs::read(&reference)
                        .map_err(|e| format!("reference {}: {e}", reference.display()))?,
                )?;
                let diff = compare_images(&Image::from(&a), &Image::from(&b), false)?;
                let mut changed = 0;
                let mut highlight = a.clone();
                if diff.dimensions_equal {
                    for (i, (x, y)) in a.pixels().iter().zip(b.pixels()).enumerate() {
                        let (xr, xg, xb) = ppl_graphics::channels(*x);
                        let (yr, yg, yb) = ppl_graphics::channels(*y);
                        if xr.abs_diff(yr) > case.expect.channel_tolerance
                            || xg.abs_diff(yg) > case.expect.channel_tolerance
                            || xb.abs_diff(yb) > case.expect.channel_tolerance
                        {
                            changed += 1;
                            highlight.pixels_mut()[i] = 0xff00ff;
                        }
                    }
                } else {
                    changed = a.pixels().len().max(b.pixels().len());
                }
                image_difference = json!({"exact":diff,"changed_above_tolerance":changed,"channel_tolerance":case.expect.channel_tolerance,"max_changed_pixels":case.expect.max_changed_pixels});
                if !diff.dimensions_equal || changed > case.expect.max_changed_pixels {
                    std::fs::create_dir_all(artifacts).map_err(|e| e.to_string())?;
                    for (suffix, image) in [("actual", &a), ("diff", &highlight)] {
                        let output = artifacts.join(format!("case-{index}-{suffix}.png"));
                        let mut guarded = protected.clone();
                        guarded.extend(references.iter().cloned());
                        protect_output(&output, &guarded)?;
                        save_image(&output, image).map_err(|e| e.to_string())?;
                        report.artifacts.push(output);
                    }
                    return Err(format!(
                        "image differs: {changed} pixels exceed channel tolerance"
                    ));
                }
                Ok(())
            })();
            if let Err(e) = comparison {
                errors.push(e);
            }
        }
        if errors.is_empty() {
            passed += 1;
        } else {
            failed += 1;
        }
        // Per-case execution screenshots live in a temporary directory. Only
        // retained references and differences belong in public artifact lists.
        actual.artifacts.retain(|p| !p.starts_with(temp.path()));
        results.push(json!({"name":case.name,"passed":errors.is_empty(),"errors":errors,"image_difference":image_difference,"result":actual}));
    }
    if failed > 0 {
        report.fail("test_failed", 1, format!("{failed} test(s) failed"));
    }
    report.data = json!({"passed":passed,"failed":failed,"cases":results});
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}
