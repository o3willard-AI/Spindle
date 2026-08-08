//! spindle-archive: Parquet archive export for compliance reports.
//!
//! Implements C11 Archive (ARC-01, ARC-02, ARC-03):
//! - `ParquetExporter` creates weekly partition directories
//! - Each directory contains: `runs.parquet`, `resource_events.parquet`,
//!   `control_results.parquet`, `nodes.parquet`, `schema.json`
//! - zstd compression
//! - Idempotent: skip if week already exported
//! - Snapshot read at start time for consistency

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{Int32Builder, StringBuilder};
use arrow::datatypes::{DataType, Field, Schema as ArrowSchema, SchemaRef};
use arrow::record_batch::RecordBatch;
use chrono::Datelike;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

/// Archive set name for a week (e.g., "2024-W01").
#[derive(Debug, Clone)]
pub struct ArchiveWeek {
    pub week: String,
    pub path: PathBuf,
}

impl ArchiveWeek {
    pub fn from_date(date: chrono::NaiveDate) -> Self {
        let iso = date.iso_week();
        let week_str = format!("{}-W{:02}", iso.year(), iso.week());
        Self {
            week: week_str.clone(),
            path: PathBuf::from(format!("archive_{}", week_str)),
        }
    }

    pub fn with_path(week: String, path: PathBuf) -> Self {
        Self { week, path }
    }

    pub fn is_exported(&self, base_dir: &Path) -> bool {
        let manifest = base_dir.join(&self.path).join("manifest.json");
        manifest.exists()
    }
}

/// Configuration for ParquetExporter.
#[derive(Debug, Clone)]
pub struct ArchiveConfig {
    pub base_dir: PathBuf,
    pub compression_level: i32,
    pub row_group_size: usize,
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            base_dir: PathBuf::from("./archive"),
            compression_level: 3,
            row_group_size: 100000,
        }
    }
}

/// Manifest written alongside Parquet files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveManifest {
    pub manifest_version: u32,
    pub archive_week: String,
    pub exported_at: String,
    pub record_counts: BTreeMap<String, usize>,
    pub file_hashes: BTreeMap<String, String>,
    pub schema_version: u32,
    pub source_raw_digests: Vec<String>,
}

/// A single column of data for Parquet writing.
enum ParquetColumn {
    String(Vec<String>),
    Int32(Vec<i32>),
}

impl ParquetColumn {
    fn len(&self) -> usize {
        match self {
            ParquetColumn::String(v) => v.len(),
            ParquetColumn::Int32(v) => v.len(),
        }
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn to_arrow_array(&self, expected_len: usize) -> Result<Arc<dyn arrow::array::Array>> {
        match self {
            ParquetColumn::String(values) => {
                let mut builder = StringBuilder::new();
                for val_idx in 0..expected_len {
                    if val_idx < values.len() && !values[val_idx].is_empty() {
                        builder.append_value(&values[val_idx]);
                    } else {
                        builder.append_null();
                    }
                }
                Ok(Arc::new(builder.finish()))
            }
            ParquetColumn::Int32(values) => {
                let mut builder = Int32Builder::new();
                for val_idx in 0..expected_len {
                    if val_idx < values.len() {
                        builder.append_value(values[val_idx]);
                    } else {
                        builder.append_null();
                    }
                }
                Ok(Arc::new(builder.finish()))
            }
        }
    }
}

/// ParquetExporter writes weekly archive partitions.
pub struct ParquetExporter {
    config: ArchiveConfig,
}

impl ParquetExporter {
    pub fn new(config: ArchiveConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &ArchiveConfig {
        &self.config
    }

    pub fn is_exported(&self, week: &ArchiveWeek) -> bool {
        week.is_exported(&self.config.base_dir)
    }

    pub fn archive_path(&self, week: &ArchiveWeek) -> PathBuf {
        self.config.base_dir.join(&week.path)
    }

    fn writer_props(&self) -> WriterProperties {
        let level = parquet::basic::ZstdLevel::try_new(self.config.compression_level)
            .unwrap_or(parquet::basic::ZstdLevel::default());
        WriterProperties::builder()
            .set_max_row_group_size(self.config.row_group_size)
            .set_compression(Compression::ZSTD(level))
            .build()
    }

    /// Write a Parquet file from columns in schema order.
    fn write_parquet(
        &self,
        file_path: &Path,
        schema: SchemaRef,
        columns: Vec<ParquetColumn>,
    ) -> Result<(String, usize)> {
        let mut buf = Vec::new();
        let props = self.writer_props();
        let mut writer = ArrowWriter::try_new(&mut buf, schema.clone(), Some(props))
            .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;

        let row_count = columns.first().map(|c| c.len()).unwrap_or(0);

        if row_count > 0 {
            let mut arrays: Vec<Arc<dyn arrow::array::Array>> = Vec::new();
            for col in &columns {
                arrays.push(col.to_arrow_array(row_count)?);
            }

            let batch = RecordBatch::try_new(schema.clone(), arrays)
                .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;
            writer
                .write(&batch)
                .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;
        }

        writer
            .finish()
            .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;
        drop(writer);
        std::fs::create_dir_all(file_path.parent().unwrap())
            .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;
        std::fs::write(file_path, &buf)
            .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;

        let mut hasher = Sha256::new();
        hasher.update(&buf);
        let hash = format!("sha256:{}", hex::encode(hasher.finalize()));

        Ok((hash, row_count))
    }

    /// Export a weekly archive. Idempotent — returns AlreadyExists if manifest present.
    pub fn export_week(
        &self,
        week: &ArchiveWeek,
        nodes: &[ArchiveNode],
        runs: &[ArchiveRun],
        resource_events: &[ArchiveResourceEvent],
        control_results: &[ArchiveControlResult],
        source_raw_digests: Vec<String>,
    ) -> Result<ArchiveManifest> {
        if self.is_exported(week) {
            warn!("Archive week {} already exported, skipping", week.week);
            return Err(ArchiveError::AlreadyExists(week.week.clone()));
        }

        let archive_dir = self.archive_path(week);
        std::fs::create_dir_all(&archive_dir)
            .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;

        info!(
            "Exporting archive week {} to {}",
            week.week,
            archive_dir.display()
        );

        let mut record_counts = BTreeMap::new();
        let mut file_hashes = BTreeMap::new();

        // Write nodes.parquet
        let (hash, count) = self.write_nodes(&archive_dir, nodes)?;
        file_hashes.insert("nodes.parquet".to_string(), hash);
        record_counts.insert("nodes.parquet".to_string(), count);

        // Write runs.parquet
        let (hash, count) = self.write_runs(&archive_dir, runs)?;
        file_hashes.insert("runs.parquet".to_string(), hash);
        record_counts.insert("runs.parquet".to_string(), count);

        // Write resource_events.parquet
        let (hash, count) = self.write_resource_events(&archive_dir, resource_events)?;
        file_hashes.insert("resource_events.parquet".to_string(), hash);
        record_counts.insert("resource_events.parquet".to_string(), count);

        // Write control_results.parquet
        let (hash, count) = self.write_control_results(&archive_dir, control_results)?;
        file_hashes.insert("control_results.parquet".to_string(), hash);
        record_counts.insert("control_results.parquet".to_string(), count);

        // Write schema.json
        let schema_json = schema_json();
        let schema_path = archive_dir.join("schema.json");
        let schema_str = serde_json::to_string_pretty(&schema_json)
            .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;
        std::fs::write(&schema_path, &schema_str)
            .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;

        // Write manifest
        let manifest = ArchiveManifest {
            manifest_version: 1,
            archive_week: week.week.clone(),
            exported_at: chrono::Utc::now().to_rfc3339(),
            record_counts,
            file_hashes,
            schema_version: 1,
            source_raw_digests,
        };

        let manifest_path = archive_dir.join("manifest.json");
        let manifest_str = serde_json::to_string_pretty(&manifest)
            .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;
        std::fs::write(&manifest_path, &manifest_str)
            .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;

        info!(
            "Archive week {} exported: {} files, manifest written",
            week.week,
            manifest.record_counts.len()
        );

        Ok(manifest)
    }

    fn write_nodes(&self, dir: &Path, nodes: &[ArchiveNode]) -> Result<(String, usize)> {
        let schema = nodes_schema();
        let file_path = dir.join("nodes.parquet");

        let row_count = nodes.len();
        let mut columns: Vec<ParquetColumn> = Vec::new();

        // Schema: id, name, platform, platform_version, chef_environment,
        //         policy_group, policy_name, last_seen, created_at
        let mut id = Vec::with_capacity(row_count);
        let mut name = Vec::with_capacity(row_count);
        let mut platform = Vec::with_capacity(row_count);
        let mut platform_ver = Vec::with_capacity(row_count);
        let mut env = Vec::with_capacity(row_count);
        let mut pg = Vec::with_capacity(row_count);
        let mut pn = Vec::with_capacity(row_count);
        let mut ls = Vec::with_capacity(row_count);
        let mut ct = Vec::with_capacity(row_count);

        for node in nodes {
            id.push(node.id.clone());
            name.push(node.name.clone());
            platform.push(node.platform.clone());
            platform_ver.push(node.platform_version.clone());
            env.push(node.chef_environment.clone());
            pg.push(node.policy_group.clone());
            pn.push(node.policy_name.clone());
            ls.push(node.last_seen.clone());
            ct.push(node.created_at.clone());
        }

        columns.push(ParquetColumn::String(id));
        columns.push(ParquetColumn::String(name));
        columns.push(ParquetColumn::String(platform));
        columns.push(ParquetColumn::String(platform_ver));
        columns.push(ParquetColumn::String(env));
        columns.push(ParquetColumn::String(pg));
        columns.push(ParquetColumn::String(pn));
        columns.push(ParquetColumn::String(ls));
        columns.push(ParquetColumn::String(ct));

        self.write_parquet(&file_path, schema, columns)
    }

    fn write_runs(&self, dir: &Path, runs: &[ArchiveRun]) -> Result<(String, usize)> {
        let schema = runs_schema();
        let file_path = dir.join("runs.parquet");

        let row_count = runs.len();
        let mut columns: Vec<ParquetColumn> = Vec::new();

        // Schema: id, node_id, run_id, status, start_time, end_time,
        //         total_resource_count, updated_count, failed_count,
        //         skipped_count, schema_version, created_at
        let mut id = Vec::with_capacity(row_count);
        let mut nid = Vec::with_capacity(row_count);
        let mut rid = Vec::with_capacity(row_count);
        let mut status = Vec::with_capacity(row_count);
        let mut st = Vec::with_capacity(row_count);
        let mut et = Vec::with_capacity(row_count);
        let mut total = Vec::with_capacity(row_count);
        let mut upd = Vec::with_capacity(row_count);
        let mut fail = Vec::with_capacity(row_count);
        let mut skip = Vec::with_capacity(row_count);
        let mut sver = Vec::with_capacity(row_count);
        let mut ct = Vec::with_capacity(row_count);

        for run in runs {
            id.push(run.id.clone());
            nid.push(run.node_id.clone());
            rid.push(run.run_id.clone());
            status.push(run.status.clone());
            st.push(run.start_time.clone());
            et.push(run.end_time.clone());
            total.push(run.total_resource_count);
            upd.push(run.updated_count);
            fail.push(run.failed_count);
            skip.push(run.skipped_count);
            sver.push(run.schema_version);
            ct.push(run.created_at.clone());
        }

        columns.push(ParquetColumn::String(id));
        columns.push(ParquetColumn::String(nid));
        columns.push(ParquetColumn::String(rid));
        columns.push(ParquetColumn::String(status));
        columns.push(ParquetColumn::String(st));
        columns.push(ParquetColumn::String(et));
        columns.push(ParquetColumn::Int32(total));
        columns.push(ParquetColumn::Int32(upd));
        columns.push(ParquetColumn::Int32(fail));
        columns.push(ParquetColumn::Int32(skip));
        columns.push(ParquetColumn::Int32(sver));
        columns.push(ParquetColumn::String(ct));

        self.write_parquet(&file_path, schema, columns)
    }

    fn write_resource_events(&self, dir: &Path, events: &[ArchiveResourceEvent]) -> Result<(String, usize)> {
        let schema = resource_events_schema();
        let file_path = dir.join("resource_events.parquet");

        let row_count = events.len();
        let mut columns: Vec<ParquetColumn> = Vec::new();

        // Schema in order: id, run_id, node_id, resource_type, resource_name,
        // action, status, duration_ms, cookbook_name, cookbook_version,
        // schema_version, created_at
        let mut id = Vec::with_capacity(row_count);
        let mut rid = Vec::with_capacity(row_count);
        let mut nid = Vec::with_capacity(row_count);
        let mut rt = Vec::with_capacity(row_count);
        let mut rn = Vec::with_capacity(row_count);
        let mut act = Vec::with_capacity(row_count);
        let mut status = Vec::with_capacity(row_count);
        let mut dur = Vec::with_capacity(row_count);
        let mut cn = Vec::with_capacity(row_count);
        let mut cv = Vec::with_capacity(row_count);
        let mut sver = Vec::with_capacity(row_count);
        let mut ct = Vec::with_capacity(row_count);

        for event in events {
            id.push(event.id.clone());
            rid.push(event.run_id.clone());
            nid.push(event.node_id.clone());
            rt.push(event.resource_type.clone());
            rn.push(event.resource_name.clone());
            act.push(event.action.clone());
            status.push(event.status.clone());
            dur.push(event.duration_ms);
            cn.push(event.cookbook_name.clone());
            cv.push(event.cookbook_version.clone());
            sver.push(event.schema_version);
            ct.push(event.created_at.clone());
        }

        columns.push(ParquetColumn::String(id));
        columns.push(ParquetColumn::String(rid));
        columns.push(ParquetColumn::String(nid));
        columns.push(ParquetColumn::String(rt));
        columns.push(ParquetColumn::String(rn));
        columns.push(ParquetColumn::String(act));
        columns.push(ParquetColumn::String(status));
        columns.push(ParquetColumn::Int32(dur));
        columns.push(ParquetColumn::String(cn));
        columns.push(ParquetColumn::String(cv));
        columns.push(ParquetColumn::Int32(sver));
        columns.push(ParquetColumn::String(ct));

        self.write_parquet(&file_path, schema, columns)
    }

    fn write_control_results(&self, dir: &Path, results: &[ArchiveControlResult]) -> Result<(String, usize)> {
        let schema = control_results_schema();
        let file_path = dir.join("control_results.parquet");

        let row_count = results.len();
        let mut columns: Vec<ParquetColumn> = Vec::new();

        let mut id = Vec::with_capacity(row_count);
        let mut rid = Vec::with_capacity(row_count);
        let mut nid = Vec::with_capacity(row_count);
        let mut pid = Vec::with_capacity(row_count);
        let mut cid = Vec::with_capacity(row_count);
        let mut status = Vec::with_capacity(row_count);
        let mut impact = Vec::with_capacity(row_count);
        let mut ct = Vec::with_capacity(row_count);

        for result in results {
            id.push(result.id.clone());
            rid.push(result.run_id.clone());
            nid.push(result.node_id.clone());
            pid.push(result.profile_id.clone());
            cid.push(result.control_id.clone());
            status.push(result.status.clone());
            impact.push(result.impact.clone());
            ct.push(result.created_at.clone());
        }

        columns.push(ParquetColumn::String(id));
        columns.push(ParquetColumn::String(rid));
        columns.push(ParquetColumn::String(nid));
        columns.push(ParquetColumn::String(pid));
        columns.push(ParquetColumn::String(cid));
        columns.push(ParquetColumn::String(status));
        columns.push(ParquetColumn::String(impact));
        columns.push(ParquetColumn::String(ct));

        self.write_parquet(&file_path, schema, columns)
    }
}

// ── Schema definitions ───────────────────────────────────────────────────────

fn make_schema(fields: Vec<Field>) -> SchemaRef {
    Arc::new(ArrowSchema::new(fields))
}

fn nodes_schema() -> SchemaRef {
    make_schema(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("platform", DataType::Utf8, true),
        Field::new("platform_version", DataType::Utf8, true),
        Field::new("chef_environment", DataType::Utf8, true),
        Field::new("policy_group", DataType::Utf8, true),
        Field::new("policy_name", DataType::Utf8, true),
        Field::new("last_seen", DataType::Utf8, true),
        Field::new("created_at", DataType::Utf8, true),
    ])
}

fn runs_schema() -> SchemaRef {
    make_schema(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("node_id", DataType::Utf8, false),
        Field::new("run_id", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("start_time", DataType::Utf8, false),
        Field::new("end_time", DataType::Utf8, true),
        Field::new("total_resource_count", DataType::Int32, false),
        Field::new("updated_count", DataType::Int32, false),
        Field::new("failed_count", DataType::Int32, false),
        Field::new("skipped_count", DataType::Int32, false),
        Field::new("schema_version", DataType::Int32, false),
        Field::new("created_at", DataType::Utf8, false),
    ])
}

fn resource_events_schema() -> SchemaRef {
    make_schema(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("run_id", DataType::Utf8, false),
        Field::new("node_id", DataType::Utf8, false),
        Field::new("resource_type", DataType::Utf8, false),
        Field::new("resource_name", DataType::Utf8, false),
        Field::new("action", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("duration_ms", DataType::Int32, false),
        Field::new("cookbook_name", DataType::Utf8, true),
        Field::new("cookbook_version", DataType::Utf8, true),
        Field::new("schema_version", DataType::Int32, false),
        Field::new("created_at", DataType::Utf8, false),
    ])
}

fn control_results_schema() -> SchemaRef {
    make_schema(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("run_id", DataType::Utf8, false),
        Field::new("node_id", DataType::Utf8, false),
        Field::new("profile_id", DataType::Utf8, false),
        Field::new("control_id", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("impact", DataType::Utf8, false),
        Field::new("created_at", DataType::Utf8, false),
    ])
}

fn schema_json() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "tables": {
            "nodes": {
                "columns": [
                    {"name": "id", "type": "string", "nullable": false},
                    {"name": "name", "type": "string", "nullable": false},
                    {"name": "platform", "type": "string", "nullable": true},
                    {"name": "platform_version", "type": "string", "nullable": true},
                    {"name": "chef_environment", "type": "string", "nullable": true},
                    {"name": "policy_group", "type": "string", "nullable": true},
                    {"name": "policy_name", "type": "string", "nullable": true},
                    {"name": "last_seen", "type": "string", "nullable": true},
                    {"name": "created_at", "type": "string", "nullable": true},
                ]
            },
            "runs": {
                "columns": [
                    {"name": "id", "type": "string", "nullable": false},
                    {"name": "node_id", "type": "string", "nullable": false},
                    {"name": "run_id", "type": "string", "nullable": false},
                    {"name": "status", "type": "string", "nullable": false},
                    {"name": "start_time", "type": "string", "nullable": false},
                    {"name": "end_time", "type": "string", "nullable": true},
                    {"name": "total_resource_count", "type": "int32", "nullable": false},
                    {"name": "updated_count", "type": "int32", "nullable": false},
                    {"name": "failed_count", "type": "int32", "nullable": false},
                    {"name": "skipped_count", "type": "int32", "nullable": false},
                    {"name": "schema_version", "type": "int32", "nullable": false},
                    {"name": "created_at", "type": "string", "nullable": false},
                ]
            },
            "resource_events": {
                "columns": [
                    {"name": "id", "type": "string", "nullable": false},
                    {"name": "run_id", "type": "string", "nullable": false},
                    {"name": "node_id", "type": "string", "nullable": false},
                    {"name": "resource_type", "type": "string", "nullable": false},
                    {"name": "resource_name", "type": "string", "nullable": false},
                    {"name": "action", "type": "string", "nullable": false},
                    {"name": "status", "type": "string", "nullable": false},
                    {"name": "duration_ms", "type": "int32", "nullable": false},
                    {"name": "cookbook_name", "type": "string", "nullable": true},
                    {"name": "cookbook_version", "type": "string", "nullable": true},
                    {"name": "schema_version", "type": "int32", "nullable": false},
                    {"name": "created_at", "type": "string", "nullable": false},
                ]
            },
            "control_results": {
                "columns": [
                    {"name": "id", "type": "string", "nullable": false},
                    {"name": "run_id", "type": "string", "nullable": false},
                    {"name": "node_id", "type": "string", "nullable": false},
                    {"name": "profile_id", "type": "string", "nullable": false},
                    {"name": "control_id", "type": "string", "nullable": false},
                    {"name": "status", "type": "string", "nullable": false},
                    {"name": "impact", "type": "string", "nullable": false},
                    {"name": "created_at", "type": "string", "nullable": false},
                ]
            },
        }
    })
}

// ── Archive data types (Serializable versions of store types) ────────────────

#[derive(Debug, Clone)]
pub struct ArchiveNode {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub platform_version: String,
    pub chef_environment: String,
    pub policy_group: String,
    pub policy_name: String,
    pub last_seen: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ArchiveRun {
    pub id: String,
    pub node_id: String,
    pub run_id: String,
    pub status: String,
    pub start_time: String,
    pub end_time: String,
    pub total_resource_count: i32,
    pub updated_count: i32,
    pub failed_count: i32,
    pub skipped_count: i32,
    pub schema_version: i32,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ArchiveResourceEvent {
    pub id: String,
    pub run_id: String,
    pub node_id: String,
    pub resource_type: String,
    pub resource_name: String,
    pub action: String,
    pub status: String,
    pub duration_ms: i32,
    pub cookbook_name: String,
    pub cookbook_version: String,
    pub schema_version: i32,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ArchiveControlResult {
    pub id: String,
    pub run_id: String,
    pub node_id: String,
    pub profile_id: String,
    pub control_id: String,
    pub status: String,
    pub impact: String,
    pub created_at: String,
}

impl From<&spindle_store::Node> for ArchiveNode {
    fn from(node: &spindle_store::Node) -> Self {
        Self {
            id: node.id.to_string(),
            name: node.name.clone(),
            platform: node.platform.clone(),
            platform_version: node.platform_version.clone(),
            chef_environment: node.chef_environment.clone(),
            policy_group: node.policy_group.clone(),
            policy_name: node.policy_name.clone(),
            last_seen: node.last_seen.to_rfc3339(),
            created_at: node.created_at.to_rfc3339(),
        }
    }
}

impl From<&spindle_store::Run> for ArchiveRun {
    fn from(run: &spindle_store::Run) -> Self {
        Self {
            id: run.id.to_string(),
            node_id: run.node_id.to_string(),
            run_id: run.run_id.clone(),
            status: run.status.clone(),
            start_time: run.start_time.to_rfc3339(),
            end_time: run.end_time.map(|t| t.to_rfc3339()).unwrap_or_default(),
            total_resource_count: run.total_resource_count,
            updated_count: run.updated_count,
            failed_count: run.failed_count,
            skipped_count: run.skipped_count,
            schema_version: run.schema_version,
            created_at: run.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("write failed: {0}")]
    WriteFailed(String),
    #[error("archive already exists: {0}")]
    AlreadyExists(String),
    #[error("file not found: {0}")]
    NotFound(String),
    #[error("manifest error: {0}")]
    ManifestError(String),
}

pub type Result<T> = std::result::Result<T, ArchiveError>;

// ── Re-exports ─────────────────────────────────────────────────────────────────

pub use spindle_store::Node as StoreNode;
pub use spindle_store::Run as StoreRun;
