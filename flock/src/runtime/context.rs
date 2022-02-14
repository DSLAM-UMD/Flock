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

//! When the lambda function is called for the first time, it deserializes the
//! corresponding execution context from the cloud environment variable.

use crate::datasink::DataSinkType;
use crate::encoding::Encoding;
use crate::error::{FlockError, Result};
use crate::runtime::plan::CloudExecutionPlan;
use crate::state::*;
use datafusion::arrow::datatypes::{Schema, SchemaRef};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::physical_plan::coalesce_batches::CoalesceBatchesExec;
use datafusion::physical_plan::memory::MemoryExec;
use datafusion::physical_plan::repartition::RepartitionExec;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::physical_plan::{collect, collect_partitioned};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::task::JoinHandle;

type CloudFunctionName = String;
type GroupSize = usize;

/// Cloud environment context is a wrapper to support compression and
/// serialization.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct CloudEnvironment {
    /// Lambda execution context.
    /// `context` is the serialized version of `ExecutionContext`.
    #[serde(with = "serde_bytes")]
    pub context:  Vec<u8>,
    /// Compress `ExecutionContext` to guarantee the total size
    /// of all environment variables doesn't exceed 4 KB.
    pub encoding: Encoding,
}

/// The cloud function type.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub enum CloudFunctionType {
    /// The default cloud function type.
    /// For AWS Lambda, the default concurrency is 1000. This function type is
    /// used for partial hash aggregation and join. The data is partitioned by
    /// the hash value of the key, and each partition is forwarded and processed
    /// by a different lambda function.
    Lambda,
    /// The function belongs to a group, and the concurrency of each function in
    /// the group is **1**. Each of them executes in a different AWS Lambda
    /// instance. This function type is used for data aggregation to save all
    /// partial results in a single AWS Lambda instance.
    Group,
}

/// Next cloud function for invocation.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub enum CloudFunction {
    /// Function type: parititioned computation
    /// The next function name with concurrency > 1.
    ///
    /// If the next call type is `Lambda`, then the name it contains is the
    /// lambda function.
    Lambda(CloudFunctionName),
    /// Function type: aggregate computation
    /// The next function name with concurrency = 1. To cope with the speed
    /// and volume of data processed, the system creates a function group that
    /// contains multiple functions (names) with the same function code. When
    /// traffic increases dramatically, each query can call a function with
    /// the same code/binary but with different names to avoid delays.
    ///
    /// If the next call type is `Group`, then the current function will pick
    /// one of function names from the group as the next call according to a
    /// certain filtering strategy.
    ///
    /// The naming rule is:
    /// If the system picks `i` from the collection [0..`GroupSize`], then the
    /// next call is `CloudFunctionName`-`i`.
    Group((CloudFunctionName, GroupSize)),
    /// There is no subsequent call to the cloud function at the end.
    Sink(DataSinkType),
}

impl Default for CloudFunction {
    fn default() -> Self {
        CloudFunction::Sink(DataSinkType::Blackhole)
    }
}

/// Cloud execution context.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExecutionContext {
    /// The execution plan on cloud.
    pub plan:          CloudExecutionPlan,
    /// Cloud Function name in the current execution context.
    ///
    /// |      Cloud Function Naming Convention       |
    /// |---------------------------------------------|
    /// |  query code  -  plan index  -  group index  |
    ///
    /// - query code: the hash digest of a sql query.
    ///
    /// - plan index: the 2-digit number [00-99] indicates the index of the
    ///   subplan of the current query in the dag.
    ///
    /// - group index: the 2-digit number [00-99] indicates the index of the
    ///  function name in the group.
    ///
    /// # Example
    ///
    /// The following is the name of one cloud function generated by the query
    /// at a certain moment.
    ///
    /// SX72HzqFz1Qij4bP-00-00
    pub name:          CloudFunctionName,
    /// Lambda function name(s) for next invocation(s).
    pub next:          CloudFunction,
    /// The current state of the execution context.
    pub state_backend: Arc<dyn StateBackend>,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        ExecutionContext {
            plan:          CloudExecutionPlan::default(),
            name:          CloudFunctionName::default(),
            next:          CloudFunction::default(),
            state_backend: Arc::new(HashMapStateBackend::default()),
        }
    }
}

impl PartialEq for ExecutionContext {
    fn eq(&self, other: &ExecutionContext) -> bool {
        self.name == other.name
            && self.next == other.next
            && serde_json::to_string(&self.plan).unwrap()
                == serde_json::to_string(&other.plan).unwrap()
    }
}

impl ExecutionContext {
    /// Returns the execution plan of the current execution context.
    ///
    /// if it's `EmptyExec`, the plan is not stored in the environment
    /// variable. In this case, we need to load the plan from S3. If the
    /// plan is already loaded, then we don't need to load it again.
    pub async fn plan(&mut self) -> Result<Vec<Arc<dyn ExecutionPlan>>> {
        self.plan.get_execution_plans().await
    }

    /// Sets the execution plan of the current execution context.
    pub async fn set_plan(&mut self, plan: CloudExecutionPlan) {
        self.plan = plan;
    }

    /// Executes the physical plan.
    ///
    /// `execute` must be called after the execution of `feed_one_source` or
    /// `feed_two_source` or `feed_data_sources`.
    pub async fn execute(&mut self) -> Result<Vec<Vec<RecordBatch>>> {
        let tasks = self
            .plan()
            .await?
            .into_iter()
            .map(|plan| {
                tokio::spawn(async move {
                    collect(plan)
                        .await
                        .map_err(|e| FlockError::Execution(e.to_string()))
                })
            })
            .collect::<Vec<JoinHandle<Result<Vec<RecordBatch>>>>>();

        Ok(futures::future::join_all(tasks)
            .await
            .into_iter()
            .map(|r| r.unwrap().unwrap())
            .collect())
    }

    /// Executes the physical plan.
    ///
    /// `execute_partitioned` must be called after the execution of
    /// `feed_one_source` or `feed_two_source` or `feed_data_sources`.
    pub async fn execute_partitioned(&mut self) -> Result<Vec<Vec<Vec<RecordBatch>>>> {
        let tasks = self
            .plan()
            .await?
            .into_iter()
            .map(|plan| {
                tokio::spawn(async move {
                    collect_partitioned(plan)
                        .await
                        .map_err(|e| FlockError::Execution(e.to_string()))
                })
            })
            .collect::<Vec<JoinHandle<Result<Vec<Vec<RecordBatch>>>>>>();

        Ok(futures::future::join_all(tasks)
            .await
            .into_iter()
            .map(|r| r.unwrap().unwrap())
            .collect())
    }

    /// The output schema of the current execution context.
    ///
    /// # Arguments
    /// * `index` - The index of the subplan.
    pub async fn schema(&mut self, index: usize) -> Result<SchemaRef> {
        Ok(self.plan.get_execution_plans().await?[index].schema())
    }

    /// Clean the data source in the given context.
    pub async fn clean_data_sources(&mut self) -> Result<()> {
        // Breadth-first search
        let mut queue = VecDeque::new();
        self.plan().await?.into_iter().for_each(|plan| {
            queue.push_back(plan);
        });

        while !queue.is_empty() {
            let mut plan = queue.pop_front().unwrap();
            if plan.children().is_empty() {
                let schema = plan.schema().clone();
                unsafe {
                    Arc::get_mut_unchecked(&mut plan)
                        .as_mut_any()
                        .downcast_mut::<MemoryExec>()
                        .unwrap()
                        .set_partitions(vec![vec![RecordBatch::new_empty(schema)]]);
                }
            }

            plan.children()
                .iter()
                .enumerate()
                .for_each(|(i, _)| queue.push_back(plan.children()[i].clone()));
        }

        Ok(())
    }

    /// Feeds all data sources to the execution plan.
    pub async fn feed_data_sources(
        &mut self,
        mut sources: Vec<Vec<Vec<RecordBatch>>>,
    ) -> Result<()> {
        // Breadth-first search
        let mut queue = VecDeque::new();
        self.plan().await?.into_iter().for_each(|plan| {
            queue.push_back(plan);
        });

        let num_partitions = sources[0].len();
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

                    if compare_schema(plan.schema(), schema) {
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
                } else {
                    let batches = (0..num_partitions)
                        .map(|_| RecordBatch::new_empty(plan.schema()))
                        .collect::<Vec<RecordBatch>>();
                    unsafe {
                        Arc::get_mut_unchecked(&mut plan)
                            .as_mut_any()
                            .downcast_mut::<MemoryExec>()
                            .unwrap()
                            .set_partitions(vec![batches]);
                    }
                }
            }

            plan.children()
                .iter()
                .enumerate()
                .for_each(|(i, _)| queue.push_back(plan.children()[i].clone()));
        }

        Ok(())
    }

    /// Checks whether the execution plan needs to be shuffled.
    pub async fn is_shuffling(&self) -> Result<bool> {
        assert!(!self.plan.execution_plans.is_empty());
        Ok(self.plan.execution_plans.iter().all(|p| {
            p.as_any().downcast_ref::<CoalesceBatchesExec>().is_some()
                && !p.children().is_empty()
                && p.children()
                    .iter()
                    .all(|c| c.as_any().downcast_ref::<RepartitionExec>().is_some())
        }))
    }

    /// Checks whether the execution plan is the last one.
    pub async fn is_last_stage(&self) -> Result<bool> {
        match self.next {
            CloudFunction::Sink(..) => Ok(true),
            _ => Ok(false),
        }
    }

    /// Check the current function type.
    ///
    /// If the function name is "<query code>-<plan index>-<group index>",
    /// then it is a group-type function.
    /// If the function name is "<query code>-<plan index>",
    /// then it is a lambda-type function.
    pub fn is_aggregate(&self) -> bool {
        let dash_count = self.name.matches('-').count();
        if dash_count == 2 {
            true
        } else if dash_count == 1 {
            false
        } else {
            panic!("Invalid function name: {}", self.name);
        }
    }
}

/// Serializes `ExecutionContext` from client-side.
pub fn marshal(ctx: &ExecutionContext, encoding: Encoding) -> Result<String> {
    Ok(match encoding {
        Encoding::Snappy | Encoding::Lz4 | Encoding::Zstd => {
            let encoded: Vec<u8> = serde_json::to_vec(ctx)?;
            serde_json::to_string(&CloudEnvironment {
                context: encoding.compress(&encoded)?,
                encoding,
            })?
        }
        Encoding::None => serde_json::to_string(&CloudEnvironment {
            context: serde_json::to_vec(ctx)?,
            encoding,
        })?,
        _ => unimplemented!(),
    })
}

/// Deserializes `ExecutionContext` from cloud-side.
pub fn unmarshal<T>(encoded_ctx: T) -> Result<ExecutionContext>
where
    T: AsRef<str>,
{
    let env: CloudEnvironment = serde_json::from_str(encoded_ctx.as_ref())?;

    Ok(match env.encoding {
        Encoding::Snappy | Encoding::Lz4 | Encoding::Zstd => {
            let encoded = env.encoding.decompress(&env.context)?;
            serde_json::from_slice(&encoded)?
        }
        Encoding::None => serde_json::from_slice(&env.context)?,
        _ => unimplemented!(),
    })
}

/// Compare two execution plans' schemas.
/// Returns true if they are belong to the same plan node.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_batches_eq;
    use crate::error::Result;
    use datafusion::arrow::array::*;
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;
    use datafusion::datasource::MemTable;

    #[tokio::test]
    async fn feed_one_data_source() -> Result<()> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("c1", DataType::Int64, false),
            Field::new("c2", DataType::Float64, false),
            Field::new("c3", DataType::Utf8, false),
            Field::new("c4", DataType::UInt64, false),
            Field::new("c5", DataType::Utf8, false),
            Field::new("neg", DataType::Int64, false),
        ]));

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

        let mut ctx = datafusion::execution::context::ExecutionContext::new();
        let table = MemTable::try_new(schema.clone(), vec![vec![RecordBatch::new_empty(schema)]])?;

        ctx.register_table("test", Arc::new(table))?;

        let sql = "SELECT MAX(c1), MIN(c2), c3 FROM test WHERE c2 < 99 GROUP BY c3 ORDER BY c3";
        let logical_plan = ctx.create_logical_plan(sql)?;
        let logical_plan = ctx.optimize(&logical_plan)?;
        let physical_plan = ctx.create_physical_plan(&logical_plan).await?;

        // Serialize the physical plan and skip its record batches
        let plan = serde_json::to_string(&physical_plan)?;

        // Deserialize the physical plan that doesn't contain record batches
        let plan: Arc<dyn ExecutionPlan> = serde_json::from_str(&plan)?;

        // Feed record batches back to the plan
        let mut ctx = ExecutionContext {
            plan: CloudExecutionPlan::new(vec![plan], None),
            name: "test".to_string(),
            next: CloudFunction::Sink(DataSinkType::Blackhole),
            ..Default::default()
        };
        ctx.feed_data_sources(vec![vec![vec![batch]]]).await?;

        let batches = ctx.execute().await?;

        let expected = vec![
            "+--------------+--------------+----+",
            "| MAX(test.c1) | MIN(test.c2) | c3 |",
            "+--------------+--------------+----+",
            "| 90           | 92.1         | a  |",
            "| 101          | 96.4         | b  |",
            "| 91           | 95.3         | d  |",
            "+--------------+--------------+----+",
        ];

        assert_batches_eq!(&expected, &batches[0]);

        Ok(())
    }

    #[tokio::test]
    async fn feed_two_data_sources() -> Result<()> {
        let schema1 = Arc::new(Schema::new(vec![
            Field::new("a", DataType::Utf8, false),
            Field::new("b", DataType::Int32, false),
        ]));
        let schema2 = Arc::new(Schema::new(vec![
            Field::new("c", DataType::Utf8, false),
            Field::new("d", DataType::Int32, false),
        ]));

        // define data.
        let batch1 = RecordBatch::try_new(
            schema1.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "b", "c", "d"])),
                Arc::new(Int32Array::from(vec![1, 10, 10, 100])),
            ],
        )?;
        // define data.
        let batch2 = RecordBatch::try_new(
            schema2.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "b", "c", "d"])),
                Arc::new(Int32Array::from(vec![1, 10, 10, 100])),
            ],
        )?;

        let mut ctx = datafusion::execution::context::ExecutionContext::new();

        let table1 =
            MemTable::try_new(schema1.clone(), vec![vec![RecordBatch::new_empty(schema1)]])?;
        let table2 =
            MemTable::try_new(schema2.clone(), vec![vec![RecordBatch::new_empty(schema2)]])?;

        ctx.register_table("t1", Arc::new(table1))?;
        ctx.register_table("t2", Arc::new(table2))?;

        let sql = concat!(
            "SELECT a, b, d ",
            "FROM t1 JOIN t2 ON a = c ",
            "ORDER BY a ASC ",
            "LIMIT 3"
        );

        let logical_plan = ctx.create_logical_plan(sql)?;
        let logical_plan = ctx.optimize(&logical_plan)?;
        let physical_plan = ctx.create_physical_plan(&logical_plan).await?;

        // Serialize the physical plan and skip its record batches
        let plan = serde_json::to_string(&physical_plan)?;

        // Deserialize the physical plan that doesn't contain record batches
        let plan: Arc<dyn ExecutionPlan> = serde_json::from_str(&plan)?;

        // Feed record batches back to the plan
        let mut ctx = ExecutionContext {
            plan: CloudExecutionPlan::new(vec![plan], None),
            name: "test".to_string(),
            next: CloudFunction::Sink(DataSinkType::Blackhole),
            ..Default::default()
        };
        let se_json = marshal(&ctx, Encoding::default())?;
        let de_json = unmarshal(&se_json)?;
        assert_eq!(ctx, de_json);

        ctx.feed_data_sources(vec![vec![vec![batch1]], vec![vec![batch2]]])
            .await?;

        let batches = ctx.execute().await?;

        let expected = vec![
            "+---+----+----+",
            "| a | b  | d  |",
            "+---+----+----+",
            "| a | 1  | 1  |",
            "| b | 10 | 10 |",
            "| c | 10 | 10 |",
            "+---+----+----+",
        ];

        assert_batches_eq!(&expected, &batches[0]);

        Ok(())
    }
}
