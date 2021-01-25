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

use datafusion::physical_plan::{common, ExecutionPlan};

use arrow::util::pretty;
use lambda::{handler_fn, Context};
use serde_json::Value;

use std::sync::Once;

use runtime::plan::*;
use runtime::Payload;
use runtime::{exec_plan, init_plan};

type Error = Box<dyn std::error::Error + Sync + Send + 'static>;

#[tokio::main]
async fn main() -> Result<(), Error> {
    lambda::run(handler_fn(handler)).await?;
    Ok(())
}

/// Initialize the lambda function once and only once.
static INIT: Once = Once::new();

/// Empty Plan before initializing the cloud environment.
static mut PLAN: LambdaPlan = LambdaPlan::None;

async fn handler(event: Value, _: Context) -> Result<Value, Error> {
    let (schema, plan) = init_plan!(INIT, PLAN);

    let record_batch = Payload::to_batch(event);
    let result = exec_plan!(plan, vec![vec![record_batch]]);
    pretty::print_batches(&result)?;

    let payload = Payload::from(&result[0], schema);
    Ok(serde_json::to_value(&payload)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn agg1_test() {
        let data = r#"
            {"header":[16,0,0,0,12,0,26,0,24,0,23,0,4,0,8,0,12,0,0,0,32,0,0,0,136,0,0,0,0,0,0,0,0,0,0,0,0,0,0,3,3,0,10,0,24,0,12,0,8,0,4,0,10,0,0,0,76,0,0,0,16,0,0,0,5,0,0,0,0,0,0,0,0,0,0,0,3,0,0,0,5,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,5,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,5,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,7,0,0,0,0,0,0,0,0,0,0,0,8,0,0,0,0,0,0,0,8,0,0,0,0,0,0,0,40,0,0,0,0,0,0,0,48,0,0,0,0,0,0,0,8,0,0,0,0,0,0,0,56,0,0,0,0,0,0,0,40,0,0,0,0,0,0,0,96,0,0,0,0,0,0,0,8,0,0,0,0,0,0,0,104,0,0,0,0,0,0,0,24,0,0,0,0,0,0,0,128,0,0,0,0,0,0,0,8,0,0,0,0,0,0,0],"body":[255,0,0,0,0,0,0,0,90,0,0,0,0,0,0,0,100,0,0,0,0,0,0,0,91,0,0,0,0,0,0,0,101,0,0,0,0,0,0,0,92,0,0,0,0,0,0,0,255,0,0,0,0,0,0,0,102,102,102,102,102,6,87,64,205,204,204,204,204,76,87,64,51,51,51,51,51,211,87,64,154,153,153,153,153,25,88,64,0,0,0,0,0,160,88,64,255,0,0,0,0,0,0,0,0,0,0,0,1,0,0,0,2,0,0,0,3,0,0,0,4,0,0,0,5,0,0,0,97,97,97,98,98,0,0,0],"schema":{"fields":[{"name":"c1","data_type":"Int64","nullable":false,"dict_id":0,"dict_is_ordered":false},{"name":"c2","data_type":"Float64","nullable":false,"dict_id":0,"dict_is_ordered":false},{"name":"c3","data_type":"Utf8","nullable":false,"dict_id":0,"dict_is_ordered":false}],"metadata":{}}}
        "#;

        let plan_key = "PLAN_JSON";
        let plan_val = r#"
        {
            "execution_plan":"hash_aggregate_exec",
            "mode":"Partial",
            "group_expr":[
               [
                  {
                     "physical_expr":"column",
                     "name":"c3"
                  },
                  "c3"
               ]
            ],
            "aggr_expr":[
               {
                  "aggregate_expr":"max",
                  "name":"MAX(c1)",
                  "data_type":"Int64",
                  "nullable":true,
                  "expr":{
                     "physical_expr":"column",
                     "name":"c1"
                  }
               },
               {
                  "aggregate_expr":"min",
                  "name":"MIN(c2)",
                  "data_type":"Float64",
                  "nullable":true,
                  "expr":{
                     "physical_expr":"column",
                     "name":"c2"
                  }
               }
            ],
            "input":{
               "execution_plan":"memory_exec",
               "schema":{
                  "fields":[
                     {
                        "name":"c3",
                        "data_type":"Utf8",
                        "nullable":false,
                        "dict_id":0,
                        "dict_is_ordered":false
                     },
                     {
                        "name":"MAX(c1)[max]",
                        "data_type":"Int64",
                        "nullable":true,
                        "dict_id":0,
                        "dict_is_ordered":false
                     },
                     {
                        "name":"MIN(c2)[min]",
                        "data_type":"Float64",
                        "nullable":true,
                        "dict_id":0,
                        "dict_is_ordered":false
                     }
                  ],
                  "metadata":{

                  }
               },
               "projection":null
            },
            "schema":{
               "fields":[
                  {
                     "name":"c3",
                     "data_type":"Utf8",
                     "nullable":false,
                     "dict_id":0,
                     "dict_is_ordered":false
                  },
                  {
                     "name":"MAX(c1)[max]",
                     "data_type":"Int64",
                     "nullable":true,
                     "dict_id":0,
                     "dict_is_ordered":false
                  },
                  {
                     "name":"MIN(c2)[min]",
                     "data_type":"Float64",
                     "nullable":true,
                     "dict_id":0,
                     "dict_is_ordered":false
                  }
               ],
               "metadata":{

               }
            }
        }
        "#;
        std::env::set_var(plan_key, plan_val);

        let event: Value = serde_json::from_str(data).unwrap();
        handler(event, Context::default()).await.ok().unwrap();
    }
}
