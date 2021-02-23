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

//! The generic lambda function for sub-plan execution on AWS Lambda.

use aws_lambda_events::event::kafka::KafkaEvent;
use aws_lambda_events::event::kinesis::KinesisEvent;
use datafusion::physical_plan::Partitioning;
use lambda::{handler_fn, Context};
use runtime::prelude::*;
use serde_json::Value;
use std::sync::Once;

#[cfg(feature = "snmalloc")]
#[global_allocator]
static ALLOC: snmalloc_rs::SnMalloc = snmalloc_rs::SnMalloc;

/// Initializes the lambda function once and only once.
static INIT: Once = Once::new();

enum CloudFunctionContext {
    Lambda(Box<ExecutionContext>),
    Uninitialized,
}

/// Lambda execution context.
static mut EXECUTION_CONTEXT: CloudFunctionContext = CloudFunctionContext::Uninitialized;

/// Performs an initialization routine once and only once.
macro_rules! init_exec_context {
    () => {{
        unsafe {
            INIT.call_once(|| match std::env::var(&globals["context"]["name"]) {
                Ok(s) => {
                    EXECUTION_CONTEXT =
                        CloudFunctionContext::Lambda(Box::new(ExecutionContext::unmarshal(&s)));
                }
                Err(_) => {
                    panic!("No execution context in the cloud environment.");
                }
            });
            match &mut EXECUTION_CONTEXT {
                CloudFunctionContext::Lambda(ctx) => ctx,
                CloudFunctionContext::Uninitialized => panic!("Uninitialized execution context!"),
            }
        }
    }};
}

#[tokio::main]
async fn main() -> Result<()> {
    lambda::run(handler_fn(handler)).await?;
    Ok(())
}

async fn source_handler(ctx: &mut ExecutionContext, event: Value) -> Result<Value> {
    let (batch, schema) = match &ctx.datasource {
        DataSource::KinesisEvent(_) => {
            let kinesis_event: KinesisEvent = serde_json::from_value(event).unwrap();
            let batch = kinesis::to_batch(kinesis_event);
            if batch.is_empty() {
                return Err(SquirtleError::Execution("No Kinesis input!".to_owned()));
            }
            let schema = batch[0].schema();
            (batch, schema)
        }
        DataSource::KafkaEvent(_) => {
            let kafka_event: KafkaEvent = serde_json::from_value(event).unwrap();
            let batch = kafka::to_batch(kafka_event);
            if batch.is_empty() {
                return Err(SquirtleError::Execution("No Kafka input!".to_owned()));
            }
            let schema = batch[0].schema();
            (batch, schema)
        }
        _ => unimplemented!(),
    };

    match LambdaExecutor::choose_strategy(&ctx, &batch) {
        ExecutionStrategy::Centralized => {
            if batch.len() > 8 {
                ctx.feed_one_source(
                    &LambdaExecutor::repartition(vec![batch], Partitioning::RoundRobinBatch(8))
                        .await?,
                );
            } else {
                ctx.feed_one_source(&vec![batch]);
            }
            let batches = ctx.execute().await?;
            let mut uuid_builder = UuidBuilder::new(&ctx.name, 1 /* one payload */);
            Ok(Payload::from(&batches[0], schema, uuid_builder.next()))
        }
        ExecutionStrategy::Distributed => {
            unimplemented!();
        }
    }
}

async fn payload_handler(ctx: &mut ExecutionContext, event: Value) -> Result<Value> {
    let (batch, uuid) = Payload::to_batch(event);
    let schema = batch.schema();

    ctx.feed_one_source(&vec![vec![batch]]);
    let batches = ctx.execute().await?;

    Ok(Payload::from(&batches[0], schema, uuid))
}

async fn handler(event: Value, _: Context) -> Result<Value> {
    let mut ctx = init_exec_context!();

    match &ctx.datasource {
        DataSource::Payload => payload_handler(&mut ctx, event).await,
        DataSource::KinesisEvent(_) | DataSource::KafkaEvent(_) => {
            source_handler(&mut ctx, event).await
        }
        DataSource::Json => Ok(event),
        _ => unimplemented!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::physical_plan::ExecutionPlan;
    use driver::QueryFlow;
    use serde_json::json;
    use std::sync::Arc;

    #[tokio::test]
    #[ignore]
    async fn generic_lambda() -> Result<()> {
        let plan = r#"{"execution_plan":"coalesce_batches_exec","input":{"execution_plan":"
            memory_exec","schema":{"fields":[{"name":"c1","data_type":"Int64","nullable":
            true,"dict_id":0,"dict_is_ordered":false},{"name":"c2","data_type":"Float64",
            "nullable":true,"dict_id":0,"dict_is_ordered":false},{"name":"c3","data_type"
            :"Utf8","nullable":true,"dict_id":0,"dict_is_ordered":false}],"metadata":{}},
            "projection":null},"target_batch_size":16384}"#;
        let name = "hello".to_owned();
        let next =
            CloudFunction::Solo("SX72HzqFz1Qij4bP-00-2021-01-28T19:27:50.298504836Z".to_owned());
        let datasource = DataSource::Json;

        let plan: Arc<dyn ExecutionPlan> = serde_json::from_str(&plan).unwrap();
        let lambda_context = ExecutionContext {
            plan,
            name,
            next,
            datasource,
        };

        let encoded = lambda_context.marshal(Encoding::default());

        // Configures the cloud environment
        std::env::set_var(&globals["context"]["name"], encoded);

        // First lambda call
        let event = json!({
            "db": "cmsc624"
        });
        assert_eq!(
            handler(event.clone(), Context::default())
                .await
                .expect("expected Ok(_) value"),
            event
        );

        // Second lambda call
        let event = json!({
            "net": "cmsc711"
        });
        assert_eq!(
            handler(event.clone(), Context::default())
                .await
                .expect("expected Ok(_) value"),
            event
        );

        Ok(())
    }

    fn init_lambda_exec(num: usize) -> Value {
        let sql = concat!(
            "SELECT MAX(c1), MIN(c2), c3 ",
            "FROM t1 ",
            "WHERE c2 < 99 GROUP BY c3"
        );
        let table_name = "t1";

        // 1. data source
        let datasource = DataSource::kinesis();
        // 2. data schema
        let (event, schema) = test_utils::random_event(&datasource, num);
        // 3. physical plan
        let plan = test_utils::physical_plan(&schema, &sql, &table_name);

        // create query flow
        let qflow = QueryFlow::new(sql, schema, datasource, plan);

        // set environment context for the first cloud function
        test_utils::set_env_context(&qflow, qflow.dag.node_count() - 1);

        event
    }

    #[tokio::test]
    async fn centralized_execution() -> Result<()> {
        let record_num = 100;
        let event = init_lambda_exec(record_num);

        // cloud function execution
        let res = handler(event, Context::default()).await?;

        // check the result of function execution
        let (batch, uuid) = Payload::to_batch(res);
        assert!(uuid.tid.contains("8qJkskaF5yXaZ3XM"));
        assert_eq!(0, uuid.seq_num);
        assert_eq!(1, uuid.seq_len);

        println!(
            "{}",
            arrow::util::pretty::pretty_format_batches(&[batch]).unwrap(),
        );

        Ok(())
    }
}
