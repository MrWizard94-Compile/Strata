//! Optional local diagnostics. Records CPU-observed events, never GPU timestamps.
use serde::Serialize;
use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::Path,
};

use crate::streaming::UploadStats;

#[derive(Serialize)]
pub struct TraceHeader<'a> {
    pub kind: &'static str,
    pub schema_version: u32,
    pub package_version: &'static str,
    pub seed: u64,
    pub generator_version: u32,
    pub initial_time_of_day: f32,
    pub adapter: &'a str,
    pub physical_size: [u32; 2],
    pub sample_count: u32,
    pub present_mode: String,
    pub workers: usize,
    pub upload_max_meshes: usize,
    pub upload_max_bytes: usize,
    pub upload_max_step_bytes: usize,
    pub upload_max_segments: usize,
    pub upload_max_cpu_ms: f64,
    pub source_label_external: Option<String>,
    pub measurement: &'static str,
}

#[derive(Serialize)]
pub struct TraceFrame {
    pub kind: &'static str,
    pub elapsed_ms: f64,
    pub redraw_interval_ms: f64,
    pub present_interval_ms: Option<f64>,
    pub outcome: &'static str,
    pub screen: &'static str,
    pub focused: bool,
    pub eye: [f32; 3],
    pub time_of_day: f32,
    pub physical_size: [u32; 2],
    pub radius_chunks: i32,
    pub presented: u64,
    pub skipped: u64,
    pub current_chunks: usize,
    pub desired_chunks: usize,
    pub resident_meshes: usize,
    pub resident_mesh_payload_bytes: usize,
    pub resident_triangles: usize,
    pub uploads: UploadStats,
}

pub struct StreamingTrace {
    writer: BufWriter<File>,
    frames: u64,
}

impl StreamingTrace {
    pub fn create(path: &Path, header: &TraceHeader<'_>) -> Result<Self, String> {
        // create_new rejects existing files and symlinks. A failed run retains
        // its partial trace; absence of an end record means it is incomplete.
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| format!("Cannot create new streaming trace {}: {e}", path.display()))?;
        let mut trace = Self {
            writer: BufWriter::with_capacity(64 * 1024, file),
            frames: 0,
        };
        trace.write(header)?;
        trace.writer.flush().map_err(|e| e.to_string())?;
        Ok(trace)
    }

    fn write(&mut self, value: &impl Serialize) -> Result<(), String> {
        serde_json::to_writer(&mut self.writer, value).map_err(|e| e.to_string())?;
        self.writer.write_all(b"\n").map_err(|e| e.to_string())
    }

    pub fn frame(&mut self, frame: &TraceFrame) -> Result<(), String> {
        self.write(frame)?;
        self.frames += 1;
        Ok(())
    }

    pub fn finish(mut self, application_clean_exit: bool) -> Result<(), String> {
        self.write(&serde_json::json!({
            "kind": "end",
            "frames": self.frames,
            "application_clean_exit": application_clean_exit,
        }))?;
        self.writer.flush().map_err(|e| e.to_string())?;
        self.writer.get_ref().sync_all().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn trace_is_parseable_finished_and_never_overwrites_existing_evidence() {
        let path = std::env::temp_dir().join(format!(
            "strata-trace-{}-{}.jsonl",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let header = TraceHeader {
            kind: "header",
            schema_version: 2,
            package_version: env!("CARGO_PKG_VERSION"),
            seed: 7,
            generator_version: crate::world::GENERATOR_VERSION,
            initial_time_of_day: crate::daylight::MORNING,
            adapter: "test, not a GPU",
            physical_size: [1440, 900],
            sample_count: 1,
            present_mode: "test".into(),
            workers: 2,
            upload_max_meshes: 2,
            upload_max_bytes: 4 * 1024 * 1024,
            upload_max_step_bytes: 1024 * 1024,
            upload_max_segments: crate::streaming::MAX_COPY_SEGMENTS,
            upload_max_cpu_ms: 2.0,
            source_label_external: None,
            measurement: "test fixture",
        };
        let mut trace = StreamingTrace::create(&path, &header).unwrap();
        trace
            .frame(&TraceFrame {
                kind: "frame",
                elapsed_ms: 10.0,
                redraw_interval_ms: 10.0,
                present_interval_ms: None,
                outcome: "presented",
                screen: "title",
                focused: true,
                eye: [0.0; 3],
                time_of_day: crate::daylight::MORNING,
                physical_size: [1440, 900],
                radius_chunks: 4,
                presented: 1,
                skipped: 0,
                current_chunks: 0,
                desired_chunks: 294,
                resident_meshes: 0,
                resident_mesh_payload_bytes: 0,
                resident_triangles: 0,
                uploads: UploadStats::default(),
            })
            .unwrap();
        trace.finish(true).unwrap();
        let before = std::fs::read(&path).unwrap();
        let lines: Vec<serde_json::Value> = std::str::from_utf8(&before)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["seed"], 7);
        assert_eq!(lines[1]["outcome"], "presented");
        assert!(lines[1]["present_interval_ms"].is_null());
        assert_eq!(lines[2]["frames"], 1);
        assert_eq!(lines[2]["application_clean_exit"], true);
        assert!(StreamingTrace::create(&path, &header).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_file(path).unwrap();
    }
}
