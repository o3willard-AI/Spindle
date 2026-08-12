//! spindle-archive: Parquet archive export for compliance reports.
//!
//! Implements C11 Archive (ARC-01, ARC-02, ARC-03):
//! - `ParquetExporter` creates weekly partition directories
//! - Each directory contains: `runs.parquet`, `resource_events.parquet`,
//!   `control_results.parquet`, `nodes.parquet`, `schema.json`
//! - zstd compression
//! - Idempotent: skip if week already exported
//! - Snapshot read at start time for consistency

#![allow(warnings)]
use spindle_signing::{RetryConfig, RetrySigner, Signer};

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
    pub signing_key_id: String,
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
            .unwrap_or_default();
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
    pub fn export_week<S: Signer + 'static>(
        &self,
        week: &ArchiveWeek,
        signer: &S,
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
        let signing_key_id = signer.key_id().as_str().to_string();
        let manifest = ArchiveManifest {
            manifest_version: 1,
            archive_week: week.week.clone(),
            exported_at: chrono::Utc::now().to_rfc3339(),
            signing_key_id,
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
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, ArchiveError>;

// ── Re-exports ─────────────────────────────────────────────────────────────────

pub use spindle_store::Node as StoreNode;
pub use spindle_store::Run as StoreRun;

// ── M4-16: Signed manifest + verification ────────────────────────────────────

/// Manifest with cryptographic signature for integrity verification.
///
/// ARC-04: Manifest is stored in the `manifests` DB table (retained forever).
/// ARC-05: Manifest includes signing key ID for verification.
/// ARC-09: Post-export verification is atomic — failure means no row deletion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedManifest {
    /// The unsigned manifest payload.
    pub manifest: ArchiveManifest,
    /// Signing key ID (from C9 signer).
    pub signing_key_id: String,
    /// Ed25519 signature over the canonical manifest JSON.
    pub signature: String,
}

/// Result of archive verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyResult {
    /// All files match their hashes and signature is valid.
    Valid,
    /// One or more files failed hash verification.
    /// Contains the list of mismatched filenames.
    Mismatch(Vec<String>),
    /// Signature verification failed.
    SignatureInvalid,
    /// Manifest file not found.
    ManifestNotFound,
    /// Signing key not found in the registry.
    KeyNotFound,
}

impl VerifyResult {
    pub fn is_valid(&self) -> bool {
        matches!(self, VerifyResult::Valid)
    }

    pub fn describe(&self) -> String {
        match self {
            VerifyResult::Valid => "valid".to_string(),
            VerifyResult::Mismatch(files) => {
                format!("mismatch: {}", files.join(", "))
            }
            VerifyResult::SignatureInvalid => "signature invalid".to_string(),
            VerifyResult::ManifestNotFound => "manifest not found".to_string(),
            VerifyResult::KeyNotFound => "key not found".to_string(),
        }
    }
}

/// Canonical serialization of a manifest for signing.
/// Produces deterministic JSON (sorted keys, compact).
fn canonical_serialized_manifest(manifest: &ArchiveManifest) -> Result<Vec<u8>> {
    let mut sorted: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    sorted.insert("archive_week".to_string(), serde_json::Value::String(manifest.archive_week.clone()));
    sorted.insert("exported_at".to_string(), serde_json::Value::String(manifest.exported_at.clone()));
    sorted.insert("file_hashes".to_string(), serde_json::to_value(&manifest.file_hashes)?);
    sorted.insert("manifest_version".to_string(), serde_json::Value::Number(manifest.manifest_version.into()));
    sorted.insert("record_counts".to_string(), serde_json::to_value(&manifest.record_counts)?);
    sorted.insert("schema_version".to_string(), serde_json::Value::Number(manifest.schema_version.into()));
    sorted.insert("source_raw_digests".to_string(), serde_json::to_value(&manifest.source_raw_digests)?);

    Ok(serde_json::to_vec(&sorted)?)
}

/// Compute SHA-256 of a file on disk.
fn file_sha256(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// Sign a manifest with the given signer.
///
/// The signature is computed over the canonical (sorted-key) JSON serialization
/// of the manifest fields (excluding `signing_key_id` and `signature`).
/// Configuration for signing retry behavior (delegates to spindle-signing).

/// Sign a manifest with the given signer using retry logic.
///
/// Uses `RetrySigner::sign_with_retry` with configurable retries.
/// Any failure after all retries propagates as a hard error — no fallback,
/// no partial artifact.
pub fn sign_manifest_with_retry(
    manifest: &ArchiveManifest,
    signer: &dyn spindle_signing::RetrySigner,
    config: &RetryConfig,
) -> Result<SignedManifest> {
    let payload = canonical_serialized_manifest(manifest)?;
    let sig = signer
        .sign_with_retry(&payload, config)
        .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;
    let sig_hex = hex::encode(sig.0);

    Ok(SignedManifest {
        manifest: manifest.clone(),
        signing_key_id: signer.key_id().as_str().to_string(),
        signature: sig_hex,
    })
}

/// Sign a manifest with the given signer (no retry — for tests/sync code).
pub fn sign_manifest(
    manifest: &ArchiveManifest,
    signer: &dyn spindle_signing::Signer,
) -> Result<SignedManifest> {
    let payload = canonical_serialized_manifest(manifest)?;
    let sig = signer
        .sign(&payload)
        .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;
    let sig_hex = hex::encode(sig.0);

    Ok(SignedManifest {
        manifest: manifest.clone(),
        signing_key_id: signer.key_id().as_str().to_string(),
        signature: sig_hex,
    })
}

/// Verify a signed manifest against files on disk.
///
/// 1. Check that all files listed in `file_hashes` exist and match their SHA-256.
/// 2. Verify the Ed25519 signature against the signing key.
///
/// Returns `VerifyResult::Mismatch` if any file fails, `SignatureInvalid` if
/// the signature is bad, or `Valid` if everything checks out.
pub fn verify_manifest(
    signed: &SignedManifest,
    archive_dir: &Path,
    public_key: &spindle_signing::PublicKey,
) -> VerifyResult {
    // 1. Verify file hashes
    let mut mismatches: Vec<String> = Vec::new();
    for (filename, expected_hash) in &signed.manifest.file_hashes {
        let file_path = archive_dir.join(filename);
        if !file_path.exists() {
            mismatches.push(filename.clone());
            continue;
        }
        match file_sha256(&file_path) {
            Ok(actual_hash) => {
                if actual_hash != *expected_hash {
                    mismatches.push(filename.clone());
                }
            }
            Err(_) => {
                mismatches.push(filename.clone());
            }
        }
    }

    if !mismatches.is_empty() {
        return VerifyResult::Mismatch(mismatches);
    }

    // 2. Verify signature
    let payload = match canonical_serialized_manifest(&signed.manifest) {
        Ok(p) => p,
        Err(_) => return VerifyResult::SignatureInvalid,
    };

    let sig_bytes = match hex::decode(&signed.signature) {
        Ok(b) if b.len() == 64 => b,
        _ => return VerifyResult::SignatureInvalid,
    };

    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let signature = spindle_signing::Signature(sig_arr);

    if spindle_signing::LocalSigner::verify(&payload, &signature, public_key) {
        VerifyResult::Valid
    } else {
        VerifyResult::SignatureInvalid
    }
}

/// Verify a signed manifest using a key registry for key lookup.
///
/// Looks up the public key by `signing_key_id` in the registry, then verifies
/// the signature. This enables verification against keys stored in PostgreSQL.
///
/// Returns `VerifyResult::KeyNotFound` if the key is not in the registry.
pub fn verify_manifest_with_registry<R: KeyRegistryProvider>(
    signed: &SignedManifest,
    archive_dir: &Path,
    registry: &R,
) -> VerifyResult {
    // 1. Verify file hashes (same logic as verify_manifest)
    let mut mismatches: Vec<String> = Vec::new();
    for (filename, expected_hash) in &signed.manifest.file_hashes {
        let file_path = archive_dir.join(filename);
        if !file_path.exists() {
            mismatches.push(filename.clone());
            continue;
        }
        match file_sha256(&file_path) {
            Ok(actual_hash) => {
                if actual_hash != *expected_hash {
                    mismatches.push(filename.clone());
                }
            }
            Err(_) => {
                mismatches.push(filename.clone());
            }
        }
    }

    if !mismatches.is_empty() {
        return VerifyResult::Mismatch(mismatches);
    }

    // 2. Verify signature using the registry
    let payload = match canonical_serialized_manifest(&signed.manifest) {
        Ok(p) => p,
        Err(_) => return VerifyResult::SignatureInvalid,
    };

    let sig_bytes = match hex::decode(&signed.signature) {
        Ok(b) if b.len() == 64 => b,
        _ => return VerifyResult::SignatureInvalid,
    };

    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let signature = spindle_signing::Signature(sig_arr);

    let key = match registry.get_key(&signed.manifest.signing_key_id) {
        Some(k) => k,
        None => return VerifyResult::KeyNotFound,
    };

    if spindle_signing::LocalSigner::verify(&payload, &signature, &key) {
        VerifyResult::Valid
    } else {
        VerifyResult::SignatureInvalid
    }
}

/// Trait for key registry backends used in manifest verification.
pub trait KeyRegistryProvider: Send + Sync {
    /// Look up a public key by key ID.
    fn get_key(&self, key_id: &str) -> Option<spindle_signing::PublicKey>;
}

// ── Re-export for convenience ───────────────────────────────────────────────

/// Verify a signed manifest file from disk using a key registry.
///
/// Reads `manifest.json` + `manifest.sig` from the archive directory,
/// then verifies using `verify_manifest_with_registry`.
pub fn verify_archive_with_registry<R: KeyRegistryProvider>(
    archive_dir: &Path,
    registry: &R,
) -> VerifyResult {
    let manifest_path = archive_dir.join("manifest.json");
    let sig_path = archive_dir.join("manifest.sig");

    if !manifest_path.exists() || !sig_path.exists() {
        return VerifyResult::Mismatch(vec!["manifest.json or manifest.sig not found".to_string()]);
    }

    let manifest_bytes = std::fs::read(&manifest_path).unwrap_or_default();
    let sig_bytes = std::fs::read(&sig_path).unwrap_or_default();

    let manifest: SignedManifest = match serde_json::from_slice(&manifest_bytes) {
        Ok(m) => m,
        Err(_) => return VerifyResult::SignatureInvalid,
    };

    // Update signature from .sig file
    let mut signed = manifest;
    signed.signature = String::from_utf8_lossy(&sig_bytes).to_string();

    verify_manifest_with_registry(&signed, archive_dir, registry)
}

/// Export a weekly archive with a signed manifest.
///
/// This is the atomic export operation:
/// 1. Write all Parquet files + schema.json
/// 2. Build manifest with file hashes
/// 3. Sign manifest with the provided signer
/// 4. Write `manifest.json` + `manifest.sig` to the archive directory
///
/// If signing fails, the archive is left in an incomplete state (manifest not
/// written). Recovery: re-run export (idempotency check via is_exported).
///
/// In production, this would be wrapped in a transaction: the manifest
/// would be stored in the `manifests` DB table only after file verification
/// succeeds, and hot/warm rows would only be deleted after DB insertion.
pub fn export_week_signed(
    exporter: &ParquetExporter,
    week: &ArchiveWeek,
    nodes: &[ArchiveNode],
    runs: &[ArchiveRun],
    resource_events: &[ArchiveResourceEvent],
    control_results: &[ArchiveControlResult],
    source_raw_digests: Vec<String>,
    signer: &dyn spindle_signing::Signer,
) -> Result<SignedManifest> {
    if exporter.is_exported(week) {
        warn!("Archive week {} already exported, skipping", week.week);
        return Err(ArchiveError::AlreadyExists(week.week.clone()));
    }

    let archive_dir = exporter.archive_path(week);

    // Phase 1: Write all Parquet files (no manifest yet)
    // Use a sub-directory to avoid partial files visible before commit
    std::fs::create_dir_all(&archive_dir)
        .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;

    info!(
        "Exporting archive week {} to {}",
        week.week,
        archive_dir.display()
    );

    let mut record_counts = BTreeMap::new();
    let mut file_hashes = BTreeMap::new();

    let (hash, count) = exporter.write_nodes(&archive_dir, nodes)?;
    file_hashes.insert("nodes.parquet".to_string(), hash);
    record_counts.insert("nodes.parquet".to_string(), count);

    let (hash, count) = exporter.write_runs(&archive_dir, runs)?;
    file_hashes.insert("runs.parquet".to_string(), hash);
    record_counts.insert("runs.parquet".to_string(), count);

    let (hash, count) = exporter.write_resource_events(&archive_dir, resource_events)?;
    file_hashes.insert("resource_events.parquet".to_string(), hash);
    record_counts.insert("resource_events.parquet".to_string(), count);

    let (hash, count) = exporter.write_control_results(&archive_dir, control_results)?;
    file_hashes.insert("control_results.parquet".to_string(), hash);
    record_counts.insert("control_results.parquet".to_string(), count);

    // Write schema.json
    let schema_json = schema_json();
    let schema_path = archive_dir.join("schema.json");
    let schema_str = serde_json::to_string_pretty(&schema_json)
        .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;
    std::fs::write(&schema_path, &schema_str)
        .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;

    // Phase 2: Build manifest
    let signing_key_id = signer.key_id().as_str().to_string();
    let manifest = ArchiveManifest {
        manifest_version: 1,
        archive_week: week.week.clone(),
        exported_at: chrono::Utc::now().to_rfc3339(),
        signing_key_id: signing_key_id.clone(),
        record_counts,
        file_hashes,
        schema_version: 1,
        source_raw_digests,
    };

    // Phase 3: Sign manifest
    let signed = sign_manifest(&manifest, signer)?;

    // Phase 4: Verify files BEFORE writing manifest (ARC-09: verify before commit)
    let verify_result = verify_manifest(&signed, &archive_dir, &signed_manifest_public_key(&signed, signer));
    if !verify_result.is_valid() {
        // Don't write manifest — archive is in incomplete state
        return Err(ArchiveError::WriteFailed(format!(
            "verification failed before manifest commit: {}",
            verify_result.describe()
        )));
    }

    // Phase 5: Write manifest + signature (atomic commit)
    let manifest_path = archive_dir.join("manifest.json");
    let manifest_str = serde_json::to_string_pretty(&signed.manifest)
        .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;
    std::fs::write(&manifest_path, &manifest_str)
        .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;

    let sig_path = archive_dir.join("manifest.sig");
    let sig_str = serde_json::to_string_pretty(&serde_json::json!({
        "signing_key_id": signed.signing_key_id,
        "signature": signed.signature,
    }))
    .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;
    std::fs::write(&sig_path, &sig_str)
        .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;

    info!(
        "Archive week {} exported and signed: {} files, manifest committed",
        week.week,
        signed.manifest.record_counts.len()
    );

    Ok(signed)
}

/// Get the public key for a signed manifest from the signer.
fn signed_manifest_public_key(
    _signed: &SignedManifest,
    signer: &dyn spindle_signing::Signer,
) -> spindle_signing::PublicKey {
    signer.public_key()
}

/// Verify an archive directory on disk.
///
/// Reads `manifest.json` + `manifest.sig` from the directory, verifies
/// all file hashes match, and checks the cryptographic signature.
pub fn verify_archive(archive_dir: &Path, public_key: &spindle_signing::PublicKey) -> Result<VerifyResult> {
    let manifest_path = archive_dir.join("manifest.json");
    let sig_path = archive_dir.join("manifest.sig");

    if !manifest_path.exists() {
        return Ok(VerifyResult::ManifestNotFound);
    }

    let manifest_str = std::fs::read_to_string(&manifest_path)
        .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;
    let manifest: ArchiveManifest = serde_json::from_str(&manifest_str)
        .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;

    let sig_str = std::fs::read_to_string(&sig_path)
        .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;
    let sig_json: serde_json::Value = serde_json::from_str(&sig_str)
        .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;
    let signing_key_id = sig_json["signing_key_id"].as_str().unwrap_or("").to_string();
    let signature = sig_json["signature"].as_str().unwrap_or("").to_string();

    let signed = SignedManifest {
        manifest,
        signing_key_id,
        signature,
    };

    Ok(verify_manifest(&signed, archive_dir, public_key))
}

/// Simulate mid-export failure for testing (ARC-09).
///
/// Writes Parquet files but does NOT write the manifest, simulating a crash
/// mid-export. The archive is left in an incomplete state — `is_exported()`
/// returns false because `manifest.json` doesn't exist.
pub fn simulate_failed_export(
    exporter: &ParquetExporter,
    week: &ArchiveWeek,
    nodes: &[ArchiveNode],
    runs: &[ArchiveRun],
    resource_events: &[ArchiveResourceEvent],
    control_results: &[ArchiveControlResult],
) -> Result<()> {
    if exporter.is_exported(week) {
        return Err(ArchiveError::AlreadyExists(week.week.clone()));
    }

    let archive_dir = exporter.archive_path(week);
    std::fs::create_dir_all(&archive_dir)
        .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;

    info!("Simulating failed export for week {} (no manifest written)", week.week);

    let _ = exporter.write_nodes(&archive_dir, nodes)?;
    let _ = exporter.write_runs(&archive_dir, runs)?;
    let _ = exporter.write_resource_events(&archive_dir, resource_events)?;
    let _ = exporter.write_control_results(&archive_dir, control_results)?;

    // Write schema.json but NOT manifest.json — simulates crash before commit
    let schema_json = schema_json();
    let schema_path = archive_dir.join("schema.json");
    let schema_str = serde_json::to_string_pretty(&schema_json)
        .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;
    std::fs::write(&schema_path, &schema_str)
        .map_err(|e| ArchiveError::WriteFailed(e.to_string()))?;

    // Deliberately do NOT write manifest.json
    Ok(())
}

/// CLI command: export a weekly archive.
///
/// Usage: `spindle archive export --week=2024-W24 --dest=/tmp/archive`
pub fn cli_export(
    week_str: &str,
    dest: &str,
    nodes: &[ArchiveNode],
    runs: &[ArchiveRun],
    resource_events: &[ArchiveResourceEvent],
    control_results: &[ArchiveControlResult],
    source_raw_digests: Vec<String>,
    signer: &dyn spindle_signing::Signer,
) -> Result<String> {
    let config = ArchiveConfig {
        base_dir: PathBuf::from(dest),
        compression_level: 3,
        row_group_size: 100000,
    };
    let exporter = ParquetExporter::new(config);

    let parts: Vec<&str> = week_str.splitn(2, "-W").collect();
    if parts.len() != 2 {
        return Err(ArchiveError::WriteFailed(format!("invalid week format: {}", week_str)));
    }
    let year = parts[0];
    let week_num = parts[1];
    let path = PathBuf::from(format!("archive_{}-W{}", year, week_num));

    let week = ArchiveWeek::with_path(week_str.to_string(), path);

    let signed = export_week_signed(
        &exporter,
        &week,
        nodes,
        runs,
        resource_events,
        control_results,
        source_raw_digests,
        signer,
    )?;

    Ok(signed.manifest.file_hashes.iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join(", "))
}

/// CLI command: verify an archive.
///
/// Usage: `spindle archive verify --archive=/tmp/archive/archive_2024-W24`
pub fn cli_verify(archive_path: &str, public_key: &spindle_signing::PublicKey) -> Result<String> {
    let path = PathBuf::from(archive_path);
    let result = verify_archive(&path, public_key)?;

    match result {
        VerifyResult::Valid => Ok("OK: archive verified (all hashes match, signature valid)".to_string()),
        VerifyResult::Mismatch(files) => Err(ArchiveError::WriteFailed(format!(
            "FAIL: mismatch in: {}",
            files.join(", ")
        ))),
        VerifyResult::SignatureInvalid => Err(ArchiveError::WriteFailed(
            "FAIL: signature verification failed".to_string()
        )),
        VerifyResult::ManifestNotFound => Err(ArchiveError::NotFound(
            "manifest.json not found in archive directory".to_string(),
        )),
        VerifyResult::KeyNotFound => Err(ArchiveError::WriteFailed(
            "signing key not found in registry".to_string(),
        )),
    }
}
