// Copyright (c) 2020-present, UMD Database Group.
//
// This program is free software: you can use, redistribute, and/or modify
// it under the terms of the GNU Affero General Public License, version 3
// or later ("AGPL"), as published by the Free Software Foundation.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or
// FITNESS FOR A PARTICULAR PURPOSE.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program. If not, see <http://www.gnu.org/licenses/>.

//! This crate responsibles for executing queries on the local machine.

use crate::error::{FlockError, Result};
use crate::launcher::{ExecutionMode, Launcher};
use crate::query::Query;
use async_trait::async_trait;
use datafusion::arrow::datatypes::{Schema, SchemaRef};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::physical_plan::collect;
use datafusion::physical_plan::memory::MemoryExec;
use datafusion::physical_plan::ExecutionPlan;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::sync::Arc;

/// LocalLauncher executes the query locally.
pub struct LocalLauncher {
    /// The physical plan of the query.
    execution_plan: Arc<dyn ExecutionPlan>,
}

#[async_trait]
impl Launcher for LocalLauncher {
    async fn new(query: &Query) -> Result<Self>
    where
        Self: Sized,
    {
        Ok(LocalLauncher {
            execution_plan: query.plan().unwrap(),
        })
    }

    fn deploy(&mut self) -> Result<()> {
        Err(FlockError::Internal(
            "Local execution doesn't require a deployment.".to_owned(),
        ))
    }

    async fn execute(&self, mode: ExecutionMode) -> Result<Vec<RecordBatch>> {
        assert!(mode == ExecutionMode::Centralized);
        collect(self.execution_plan.clone())
            .await
            .map_err(|e| FlockError::Execution(e.to_string()))
    }
}

impl LocalLauncher {
    /// Compare two execution plans' schemas.
    /// Returns true if they are belong to the same plan node.
    ///
    /// # Arguments
    /// * `schema1` - The first schema.
    /// * `schema2` - The second schema.
    ///
    /// # Returns
    /// * `true` - If the schemas belong to the same plan node.
    fn compare_schema(schema1: SchemaRef, schema2: SchemaRef) -> bool {
        let (superset, subset) = if schema1.fields().len() >= schema2.fields().len() {
            (schema1, schema2)
        } else {
            (schema2, schema1)
        };

        let fields = superset
            .fields()
            .iter()
            .map(|f| f.name())
            .collect::<HashSet<_>>();

        subset.fields().iter().all(|f| fields.contains(&f.name()))
    }

    /// Feeds the query with data.
    ///
    /// # Arguments
    /// * `sources` - A list of data sources.
    pub fn feed_data_sources(&mut self, mut sources: Vec<Vec<Vec<RecordBatch>>>) {
        // Breadth-first search
        let mut queue = VecDeque::new();
        queue.push_back(self.execution_plan.clone());

        let mut found = false;
        let mut index = 0xFFFFFFFF;
        while !queue.is_empty() {
            let mut plan = queue.pop_front().unwrap();
            if plan.children().is_empty() {
                for (i, partition) in sources.iter().enumerate() {
                    let mut schema = Arc::new(Schema::new(vec![]));
                    let mut flag = false;
                    for p in partition.iter().filter(|p| !p.is_empty()) {
                        if let Some(b) = p.iter().next() {
                            schema = b.schema();
                            flag = true;
                            break;
                        }
                    }
                    if !flag {
                        continue;
                    }

                    if LocalLauncher::compare_schema(plan.schema(), schema) {
                        index = i;
                        found = true;
                        break;
                    }
                }

                if found {
                    unsafe {
                        Arc::get_mut_unchecked(&mut plan)
                            .as_mut_any()
                            .downcast_mut::<MemoryExec>()
                            .unwrap()
                            .set_partitions(sources.remove(index));
                        index = 0xFFFFFFFF;
                        found = false;
                    }
                }
            }

            plan.children()
                .iter()
                .enumerate()
                .for_each(|(i, _)| queue.push_back(plan.children()[i].clone()));
        }
    }

    /// Collects the results of the query.
    pub async fn collect(&self) -> Result<Vec<RecordBatch>> {
        collect(self.execution_plan.clone())
            .await
            .map_err(|e| FlockError::Execution(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_batches_eq;
    use crate::datasink::DataSinkType;
    use crate::datasource::DataSource;
    use crate::query::QueryType;
    use crate::query::Table;
    use crate::state::*;
    use datafusion::arrow::array::*;
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;

    #[tokio::test]
    async fn version_check() -> Result<()> {
        let manifest = cargo_toml::Manifest::from_str(include_str!("../../Cargo.toml")).unwrap();
        assert_eq!(env!("CARGO_PKG_VERSION"), manifest.package.unwrap().version);
        Ok(())
    }

    #[tokio::test]
    async fn local_launcher() -> Result<()> {
        let table_name = "test_table".to_owned();
        let schema = Arc::new(Schema::new(vec![
            Field::new("c1", DataType::Int64, false),
            Field::new("c2", DataType::Float64, false),
            Field::new("c3", DataType::Utf8, false),
            Field::new("c4", DataType::UInt64, false),
            Field::new("c5", DataType::Utf8, false),
            Field::new("neg", DataType::Int64, false),
        ]));
        let sql = "SELECT MIN(c1), AVG(c4), COUNT(c3) FROM test_table";
        let query = Query::new(
            sql,
            vec![Table(table_name, schema.clone())],
            DataSource::Memory,
            DataSinkType::Blackhole,
            None,
            QueryType::OLAP,
            Arc::new(HashMapStateBackend::new()),
        );

        let mut launcher = LocalLauncher::new(&query).await?;

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![90, 90, 91, 101, 92, 102, 93, 103])),
                Arc::new(Float64Array::from(vec![
                    92.1, 93.2, 95.3, 96.4, 98.5, 99.6, 100.7, 101.8,
                ])),
                Arc::new(StringArray::from(vec![
                    "a", "a", "d", "b", "b", "d", "c", "c",
                ])),
                Arc::new(UInt64Array::from(vec![33, 1, 54, 33, 12, 75, 2, 87])),
                Arc::new(StringArray::from(vec![
                    "rapport",
                    "pedantic",
                    "mimesis",
                    "haptic",
                    "baksheesh",
                    "amok",
                    "devious",
                    "c",
                ])),
                Arc::new(Int64Array::from(vec![
                    -90, -90, -91, -101, -92, -102, -93, -103,
                ])),
            ],
        )?;

        launcher.feed_data_sources(vec![vec![vec![batch]]]);
        let batches = launcher.collect().await?;

        let expected = vec![
            "+--------------------+--------------------+----------------------+",
            "| MIN(test_table.c1) | AVG(test_table.c4) | COUNT(test_table.c3) |",
            "+--------------------+--------------------+----------------------+",
            "| 90                 | 37.125             | 8                    |",
            "+--------------------+--------------------+----------------------+",
        ];

        assert_batches_eq!(&expected, &batches);

        Ok(())
    }
}
