// Copyright (c) 2020-2021, UMD Database Group. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Common unit test utility methods

#![warn(missing_docs)]
// Clippy lints, some should be disabled incrementally
#![allow(
    clippy::float_cmp,
    clippy::module_inception,
    clippy::new_without_default,
    clippy::ptr_arg,
    clippy::type_complexity,
    clippy::wrong_self_convention,
    clippy::should_implement_trait,
    clippy::comparison_to_empty
)]
#![feature(get_mut_unchecked)]

use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use datafusion::datasource::MemTable;
use datafusion::execution::context::ExecutionContext;
use datafusion::physical_plan::ExecutionPlan;
use driver::QueryFlow;
use fake::{Dummy, Fake};
use runtime::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

extern crate daggy;
use daggy::NodeIndex;

mod kinesis;

/// The data record schema for unit tests.
#[derive(Dummy, Debug, Clone, PartialEq, Deserialize, Serialize)]
pub(crate) struct DataRecord {
    #[dummy(faker = "1..100")]
    pub c1: i64,
    #[dummy(faker = "1..100")]
    pub c2: i64,
    pub c3: String,
}

impl DataRecord {
    /// Return the schema of the data record.
    pub fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("c1", DataType::Int64, false),
            Field::new("c2", DataType::Int64, false),
            Field::new("c3", DataType::Utf8, false),
        ]))
    }
}

/// Compares formatted output of a record batch with an expected
/// vector of strings, with the result of pretty formatting record
/// batches. This is a macro so errors appear on the correct line
///
/// Designed so that failure output can be directly copy/pasted
/// into the test code as expected results.
///
/// Expects to be called about like this:
///
/// `assert_batch_eq!(expected_lines: &[&str], batches: &[RecordBatch])`
#[macro_export]
macro_rules! assert_batches_eq {
    ($EXPECTED_LINES: expr, $CHUNKS: expr) => {
        let expected_lines: Vec<String> = $EXPECTED_LINES.iter().map(|&s| s.into()).collect();

        let formatted = arrow::util::pretty::pretty_format_batches($CHUNKS).unwrap();

        let actual_lines: Vec<&str> = formatted.trim().lines().collect();

        assert_eq!(
            expected_lines, actual_lines,
            "\n\nexpected:\n\n{:#?}\nactual:\n\n{:#?}\n\n",
            expected_lines, actual_lines
        );
    };
}

/// Compares formatted output of a record batch with an expected
/// vector of strings in a way that order does not matter.
/// This is a macro so errors appear on the correct line
///
/// Designed so that failure output can be directly copy/pasted
/// into the test code as expected results.
///
/// Expects to be called about like this:
///
/// `assert_batch_sorted_eq!(expected_lines: &[&str], batches: &[RecordBatch])`
#[macro_export]
macro_rules! assert_batches_sorted_eq {
    ($EXPECTED_LINES: expr, $CHUNKS: expr) => {
        let mut expected_lines: Vec<String> = $EXPECTED_LINES.iter().map(|&s| s.into()).collect();

        // sort except for header + footer
        let num_lines = expected_lines.len();
        if num_lines > 3 {
            expected_lines.as_mut_slice()[2..num_lines - 1].sort_unstable()
        }

        let formatted = arrow::util::pretty::pretty_format_batches($CHUNKS).unwrap();
        // fix for windows: \r\n -->

        let mut actual_lines: Vec<&str> = formatted.trim().lines().collect();

        // sort except for header + footer
        let num_lines = actual_lines.len();
        if num_lines > 3 {
            actual_lines.as_mut_slice()[2..num_lines - 1].sort_unstable()
        }

        assert_eq!(
            expected_lines, actual_lines,
            "\n\nexpected:\n\n{:#?}\nactual:\n\n{:#?}\n\n",
            expected_lines, actual_lines
        );
    };
}

/// Generate a random Kinesis event.
///
/// You can use an AWS Lambda function to process records in an Amazon Kinesis
/// data stream. A Kinesis data stream is a set of shards. Each shard contains a
/// sequence of data records. Before invoking the function, Lambda continues to
/// read records from the stream until it has gathered a full batch, or until
/// the batch window expires.
///
/// More details: <https://docs.aws.amazon.com/lambda/latest/dg/with-kinesis.html>
///
/// # Arguments
///
/// * `num`: the number of records in the event.
///
/// # Return
///
/// A valid JSON [value](serde_json::Value) representing the Kinesis event, and
/// the schema of the data records.
///
/// # Example
///
/// Kinesis record event
///
/// ```json
/// {
///     "Records": [
///         {
///             "kinesis": {
///                 "kinesisSchemaVersion": "1.0",
///                 "partitionKey": "1",
///                 "sequenceNumber": "49590338271490256608559692538361571095921575989136588898",
///                 "data": "SGVsbG8sIHRoaXMgaXMgYSB0ZXN0Lg==",
///                 "approximateArrivalTimestamp": 1545084650.987
///             },
///             "eventSource": "aws:kinesis",
///             "eventVersion": "1.0",
///             "eventID": "shardId-000000000006:49590338271490256608559692538361571095921575989136588898",
///             "eventName": "aws:kinesis:record",
///             "invokeIdentityArn": "arn:aws:iam::123456789012:role/lambda-role",
///             "awsRegion": "us-east-2",
///             "eventSourceARN": "arn:aws:kinesis:us-east-2:123456789012:stream/lambda-stream"
///         },
///         {
///             "kinesis": {
///                 "kinesisSchemaVersion": "1.0",
///                 "partitionKey": "1",
///                 "sequenceNumber": "49590338271490256608559692540925702759324208523137515618",
///                 "data": "VGhpcyBpcyBvbmx5IGEgdGVzdC4=",
///                 "approximateArrivalTimestamp": 1545084711.166
///             },
///             "eventSource": "aws:kinesis",
///             "eventVersion": "1.0",
///             "eventID": "shardId-000000000006:49590338271490256608559692540925702759324208523137515618",
///             "eventName": "aws:kinesis:record",
///             "invokeIdentityArn": "arn:aws:iam::123456789012:role/lambda-role",
///             "awsRegion": "us-east-2",
///             "eventSourceARN": "arn:aws:kinesis:us-east-2:123456789012:stream/lambda-stream"
///         }
///     ]
/// }
/// ```
pub fn random_kinesis_event(num: usize) -> (Value, SchemaRef) {
    (
        serde_json::to_value(kinesis::KinesisEvent {
            records: fake::vec![kinesis::KinesisEventRecord; num],
        })
        .unwrap(),
        DataRecord::schema(),
    )
}

/// Generate a random event for a given data source.
///
/// # Arguments
///
/// * `datasource`: A [data source](DataSource) type.
/// * `num`: the number of records in the event.
///
/// # Return
///
/// A valid JSON [value](serde_json::Value) representing the event, and
/// the schema of the data records in the event.
pub fn random_event(datasource: &DataSource, num: usize) -> (Value, SchemaRef) {
    match &datasource {
        DataSource::KinesisEvent(_) => random_kinesis_event(num),
        DataSource::KafkaEvent(_) => unimplemented!(),
        _ => unimplemented!(),
    }
}

/// Register a table
pub fn register_table(schema: &SchemaRef, table_name: &str) -> ExecutionContext {
    let mut ctx = ExecutionContext::new();

    // create empty batch to generate the execution plan
    let batch = RecordBatch::new_empty(schema.clone());
    let table = MemTable::try_new(schema.clone(), vec![vec![batch]]).unwrap();
    ctx.register_table(table_name, Arc::new(table));

    ctx
}

/// Generate a physical plan of a given query.
///
/// # Arguments
///
/// * `schema`: the data schema.
/// * `sql`: ANSI SQL statement.
/// * `table_name`: the table name the query works on.
///
/// # Return
///
/// `Arc<dyn ExecutionPlan>`: A physical execution plan.
pub fn physical_plan(schema: &SchemaRef, sql: &str, table_name: &str) -> Arc<dyn ExecutionPlan> {
    let mut ctx = register_table(&schema, table_name);
    runtime::executor::plan::physical_plan(&mut ctx, &sql).unwrap()
}

/// Set the cloud environment context to a specific cloud function in the query.
///
/// # Parameters
///
/// * `qflow`: A struct `QueryFlow` contains all revelent query information.
/// * `idx`: the node index of DAG in the `qflow`.
pub fn set_env_context(qflow: &QueryFlow, idx: usize) {
    let ctx = &qflow.ctx[&NodeIndex::new(idx)];
    std::env::set_var(&globals["context"]["name"], ctx.marshal(Encoding::Zstd));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn random_kinesis_data() -> Result<()> {
        for i in 1..5 {
            let (value, _) = random_kinesis_event(i);
            let event: aws_lambda_events::event::kinesis::KinesisEvent =
                serde_json::from_value(value)?;
            assert_eq!(event.records.len(), i);
        }
        Ok(())
    }
}
