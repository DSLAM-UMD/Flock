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

//! QueryFlow contains all the context information of the current query
//! plan. It is responsible for deploying lambda functions and execution
//! context.

extern crate daggy;
use daggy::{NodeIndex, Walker};

use crate::datasource::DataSource;
use crate::driver::funcgen::dag::*;
use crate::prelude::*;
use blake2::{Blake2b, Digest};
use chrono::{DateTime, Utc};
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::physical_plan::ExecutionPlan;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// `QueryFlow` contains all the context information of the current query
/// plan. It is responsible for deploying lambda functions and execution
/// context.
#[derive(Debug)]
pub struct QueryFlow {
    /// A query information received from the client-side.
    pub query: Query,
    /// A DAG structure of the partitioned query plan.
    pub dag:   QueryDag,
    /// The execution context is used to initialize the execution environment.
    /// Each node in `dag` have a corresponding `ExecutionContext` in this map.
    /// TODO: This is not a good design. We can keep each context in the node
    /// itself.
    pub ctx:   HashMap<NodeIndex, ExecutionContext>,
}

impl QueryFlow {
    /// Create a new `QueryFlow` from a given query.
    pub fn new(
        sql: &str,
        schema: SchemaRef,
        datasource: DataSource,
        plan: Arc<dyn ExecutionPlan>,
    ) -> Self {
        let query = Box::new(StreamQuery {
            ansi_sql: sql.to_owned(),
            schema,
            datasource,
        });
        QueryFlow::from(query)
    }

    /// Create a new `QueryFlow` from a given query.
    pub fn from(query: Box<dyn Query>) -> QueryFlow {
        let plan = query.plan();

        let mut dag = QueryDag::from(plan);
        QueryFlow::add_source(plan, &mut dag);
        let ctx = QueryFlow::build_context(&*query, &mut dag);
        QueryFlow { query, dag, ctx }
    }

    /// Add a data source node into `QueryDag`.
    #[inline]
    fn add_source(plan: &Arc<dyn ExecutionPlan>, dag: &mut QueryDag) {
        let parent = dag.node_count() - 1;
        dag.add_child(
            NodeIndex::new(parent),
            DagNode {
                plan:        plan.clone(),
                concurrency: CONCURRENCY_1,
            },
        );
    }

    /// Return a unique function name.
    ///
    /// The function name is generated by hashing the query SQL, and then
    /// concatenating it with the subplan index in the DAG, and the current
    /// time.
    ///
    /// # Arguments
    /// * `query_code` - The hash code of the query SQL.
    /// * `node_idx` - The index of the subplan in the DAG.
    /// * `time` - The current UTC time.
    ///
    /// # Returns
    /// * The unique function name.
    #[inline]
    fn function_name(query_code: &str, node_idx: &NodeIndex, timestamp: &DateTime<Utc>) -> String {
        let plan_index = format!("{:0>2}", node_idx.index());
        format!("{}-{}-{:?}", query_code, plan_index, timestamp)
    }

    /// Create a **unique** execution context for each subplan in the DAG.
    ///
    /// The distributed dataflow execution paradigm on FaaS is implemented using
    /// the execution context. Each subplan has its own execution context, which
    /// means that each lambda function is in charge of executing the given
    /// subplan and passing the result to the next subplan.
    fn build_context(
        query: &dyn Query,
        dag: &mut QueryDag,
    ) -> HashMap<NodeIndex, ExecutionContext> {
        let mut query_code = base64::encode(&Blake2b::digest(query.sql().as_bytes()));
        query_code.truncate(16);
        let timestamp = chrono::offset::Utc::now();

        let mut ctx = HashMap::new();
        let root = NodeIndex::new(0);
        ctx.insert(
            root,
            ExecutionContext {
                plan: dag.get_node(root).unwrap().plan.clone(),
                name: QueryFlow::function_name(&query_code, &root, &timestamp),
                next: CloudFunction::Sink(DataSinkType::Blackhole), // the last function
                ..Default::default()
            },
        );

        let ncount = dag.node_count();
        assert!((1..=99).contains(&ncount));

        // Breadth-first search
        let mut queue = VecDeque::new();

        queue.push_back(root);
        while let Some(parent) = queue.pop_front() {
            for (_, node) in dag.children(parent).iter(dag) {
                ctx.insert(
                    node,
                    ExecutionContext {
                        plan: dag.get_node(node).unwrap().plan.clone(),
                        name: QueryFlow::function_name(&query_code, &node, &timestamp),
                        next: {
                            let name = ctx.get(&parent).unwrap().name.clone();
                            if dag.get_node(parent).unwrap().concurrency == 1 {
                                CloudFunction::Group((name, CONCURRENCY_8))
                            } else {
                                CloudFunction::Lambda(name)
                            }
                        },
                        ..Default::default()
                    },
                );
                queue.push_back(node);
            }
        }
        ctx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use datafusion::arrow::array::*;
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;

    use datafusion::datasource::MemTable;
    use datafusion::execution::context::ExecutionContext;

    use blake2::{Blake2b, Digest};

    async fn init_query_flow(sql: &str) -> Result<QueryFlow> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("a", DataType::Utf8, false),
            Field::new("b", DataType::Int32, false),
        ]));

        // define data.
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "b", "c", "d"])),
                Arc::new(Int32Array::from(vec![1, 10, 10, 100])),
            ],
        )?;

        let mut ctx = ExecutionContext::new();

        let table = MemTable::try_new(schema.clone(), vec![vec![batch]])?;

        ctx.register_table("t", Arc::new(table))?;

        let plan = physical_plan(&ctx, sql).await?;
        let query: Box<dyn Query> = Box::new(StreamQuery {
            ansi_sql: sql.to_string(),
            schema,
            datasource: DataSource::UnknownEvent,
            plan,
        });

        let mut dag = QueryDag::from(query.plan());
        QueryFlow::add_source(query.plan(), &mut dag);
        let ctx = QueryFlow::build_context(&*query, &mut dag);

        Ok(QueryFlow { query, dag, ctx })
    }

    fn function_name(func: &QueryFlow, idx: usize) -> Result<String> {
        Ok(func
            .ctx
            .get(&NodeIndex::new(idx))
            .ok_or_else(|| {
                FlockError::QueryStage(
                    "Failed to get function name field from the hash map".to_string(),
                )
            })?
            .name
            .clone())
    }

    fn next_function(func: &QueryFlow, idx: usize) -> Result<CloudFunction> {
        Ok(func
            .ctx
            .get(&NodeIndex::new(idx))
            .ok_or_else(|| {
                FlockError::QueryStage(
                    "Failed to get next function field from the hash map".to_string(),
                )
            })?
            .next
            .clone())
    }

    #[tokio::test]
    async fn execute_context_with_select() -> Result<()> {
        let sql = concat!("SELECT b FROM t ORDER BY b ASC LIMIT 3");

        let mut functions = init_query_flow(sql).await?;

        assert!(function_name(&functions, 0)?.contains("00"));
        assert!(function_name(&functions, 1)?.contains("01"));

        assert!(matches!(
            next_function(&functions, 0)?,
            CloudFunction::Sink(..)
        ));
        assert!(matches!(
            next_function(&functions, 1)?,
            CloudFunction::Lambda(..)
        ));

        let dag = &mut functions.dag;
        assert_eq!(2, dag.node_count());
        assert_eq!(1, dag.edge_count());

        let mut iter = dag.node_weights_mut();
        let mut node = iter.next().unwrap();
        assert!(node.get_plan_str().contains(r#"projection_exec"#));
        assert!(node.get_plan_str().contains(r#"memory_exec"#));
        assert_eq!(8, node.concurrency);

        node = iter.next().unwrap();
        assert!(node.get_plan_str().contains(r#"projection_exec"#));
        assert!(node.get_plan_str().contains(r#"memory_exec"#));
        assert_eq!(1, node.concurrency);

        Ok(())
    }

    #[tokio::test]
    async fn execute_context_with_agg() -> Result<()> {
        let sql = concat!("SELECT MIN(a), AVG(b) ", "FROM t ", "GROUP BY b");

        let mut functions = init_query_flow(sql).await?;

        assert!(function_name(&functions, 0)?.contains("00"));
        assert!(function_name(&functions, 1)?.contains("01"));
        assert!(function_name(&functions, 2)?.contains("02"));

        assert!(matches!(
            next_function(&functions, 0)?,
            CloudFunction::Sink(..)
        ));
        assert!(matches!(
            next_function(&functions, 1)?,
            CloudFunction::Group(..)
        ));
        assert!(matches!(
            next_function(&functions, 2)?,
            CloudFunction::Lambda(..)
        ));

        let dag = &mut functions.dag;
        assert_eq!(3, dag.node_count());
        assert_eq!(2, dag.edge_count());

        let mut iter = dag.node_weights_mut();
        let mut node = iter.next().unwrap();
        assert!(node.get_plan_str().contains(r#"projection_exec"#));
        assert!(node.get_plan_str().contains(r#"hash_aggregate_exec"#));
        assert!(node.get_plan_str().contains(r#"memory_exec"#));
        assert_eq!(1, node.concurrency);

        node = iter.next().unwrap();
        assert!(node.get_plan_str().contains(r#"hash_aggregate_exec"#));
        assert!(node.get_plan_str().contains(r#"memory_exec"#));
        assert_eq!(8, node.concurrency);

        node = iter.next().unwrap();
        assert!(node.get_plan_str().contains(r#"projection_exec"#));
        assert!(node.get_plan_str().contains(r#"hash_aggregate_exec"#));
        assert!(node.get_plan_str().contains(r#"hash_aggregate_exec"#));
        assert!(node.get_plan_str().contains(r#"memory_exec"#));
        assert_eq!(1, node.concurrency);

        Ok(())
    }

    #[tokio::test]
    async fn lambda_function_name() -> Result<()> {
        // The hash of the SQL statement is used as the first 16 characters of the
        // function name.
        let hash = Blake2b::digest(b"SELECT b FROM t ORDER BY b ASC LIMIT 3");
        let mut s1 = base64::encode(&hash);
        s1.truncate(16);

        // The sub-plan index in the dag is used as the second 2 characters of the
        // function name.
        let s2 = format!("{:0>2}", 0);
        //                  |||
        //                  ||+-- width
        //                  |+--- align
        //                  +---- fill
        assert_eq!("00", s2);

        // The timestamp is used as the last part of the function name.
        let s3 = chrono::offset::Utc::now();

        // Example: "SX72HzqFz1Qij4bP-00-2021-09-23T19:25:49.633392315Z"
        let name = format!("{}-{}-{:?}", s1, s2, s3);
        println!("{}", name);

        Ok(())
    }
}
