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

//! This is a FaaS baseline of the benchmark. It contains the client's
//! coordinator, and the functions communicate through S3.

#[path = "../nexmark/main.rs"]
mod nexmark_bench;
use flock::aws::lambda;
use flock::prelude::*;
use log::info;
use nexmark::register_nexmark_tables;
use nexmark_bench::*;
use serde_json::Value;
use std::collections::HashMap;
use std::time::SystemTime;
use structopt::StructOpt;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    benchmark(&mut NexmarkBenchmarkOpt::from_args()).await?;
    Ok(())
}

pub fn set_nexmark_config(opt: &mut NexmarkBenchmarkOpt) -> Result<()> {
    opt.async_type = false;
    opt.generators = 1;
    match opt.query_number {
        0..=4 | 6 | 9 | 13 => opt.seconds = 1, // ElementWise
        5 | 7..=8 | 11..=12 => opt.seconds = 10,
        _ => unreachable!(),
    };
    Ok(())
}

async fn benchmark(opt: &mut NexmarkBenchmarkOpt) -> Result<()> {
    set_nexmark_config(opt)?;
    info!(
        "Running the NEXMark benchmark [S3] with the following options: {:?}",
        opt
    );
    let nexmark_conf = create_nexmark_source(opt).await?;
    let query_number = opt.query_number;

    let mut ctx = register_nexmark_tables().await?;
    let plans = create_physical_plans(&mut ctx, query_number).await?;
    let worker = create_nexmark_functions(
        opt,
        nexmark_conf.window.clone(),
        plans.last().unwrap().clone(),
    )
    .await?;

    // The source generator function needs the metadata to determine the type of the
    // workers such as single function or a group. We don't want to keep this info
    // in the environment as part of the source function. Otherwise, we have to
    // *delete* and **recreate** the source function every time we change the query.
    let mut metadata = HashMap::new();
    metadata.insert("workers".to_string(), serde_json::to_string(&worker)?);
    metadata.insert("invocation_type".to_string(), "sync".to_string());

    let start_time = SystemTime::now();
    info!(
        "[OK] Invoking NEXMark source function: {}",
        FLOCK_DATA_SOURCE_FUNC_NAME.clone()
    );
    let payload = serde_json::to_vec(&Payload {
        datasource: DataSource::S3(nexmark_conf.clone()),
        query_number: Some(query_number),
        metadata: Some(metadata),
        ..Default::default()
    })?
    .into();

    let resp: Value = serde_json::from_slice(
        &lambda::invoke_function(
            &FLOCK_DATA_SOURCE_FUNC_NAME,
            &FLOCK_LAMBDA_SYNC_CALL,
            Some(payload),
        )
        .await?
        .payload
        .expect("No response"),
    )?;

    info!("Recieved response from the source function: {:#?}", resp);

    let function_name = resp["function"].as_str().unwrap().to_string();
    let sync = true;

    let mut metadata: HashMap<String, String> = HashMap::new();
    metadata.insert(
        "s3_bucket".to_string(),
        resp["bucket"].as_str().unwrap().to_string(),
    );
    metadata.insert(
        "s3_key".to_string(),
        resp["key"].as_str().unwrap().to_string(),
    );

    let payload = serde_json::to_vec(&Payload {
        query_number: Some(query_number),
        datasource: DataSource::Payload(sync),
        uuid: serde_json::from_str(resp["uuid"].as_str().unwrap())?,
        encoding: serde_json::from_str(resp["encoding"].as_str().unwrap())?,
        metadata: Some(metadata),
        ..Default::default()
    })?
    .into();

    info!("[OK] Invoking NEXMark worker function: {}", function_name);
    let resp: Value = serde_json::from_slice(
        &lambda::invoke_function(&function_name, &FLOCK_LAMBDA_SYNC_CALL, Some(payload))
            .await?
            .payload
            .expect("No response"),
    )?;
    info!("[OK] Received response: {:?}", resp);
    let end_time = SystemTime::now();

    info!(
        "[OK] The NEXMark benchmark [S3] took {} milliseconds",
        end_time.duration_since(start_time).unwrap().as_millis()
    );
    Ok(())
}
