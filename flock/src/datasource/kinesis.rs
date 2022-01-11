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

//! Amazon Kinesis Data Streams is a managed service that scales elastically for
//! real-time processing of streaming big data.

use aws_lambda_events::event::kinesis::KinesisEvent;

use datafusion::arrow::json::{self, reader::infer_json_schema};
use datafusion::arrow::record_batch::RecordBatch;

use crate::prelude::*;
use rayon::prelude::*;
use rusoto_core::Region;
use rusoto_kinesis::{DescribeStreamInput, Kinesis, KinesisClient};
use rusoto_lambda::CreateEventSourceMappingRequest;
use serde::{Deserialize, Serialize};
use std::io::BufReader;
use std::sync::Arc;

/// A struct to manage all Kinesis info in cloud environment.
#[derive(Default, Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct KinesisSource {
    /// The name of the Amazon Kinesis data stream.
    pub stream_name: String,
    /// The windows group stream elements by time or rows.
    pub window:      Window,
}

impl KinesisSource {
    /// Fetches data records from Kinesis Data Streams.
    pub fn fetch_data(&self) -> Result<RecordBatch> {
        unimplemented!();
    }
}

/// Creates event source mapping for Kinesis Data Streams.
pub async fn create_event_source_mapping_request(
    stream_name: &str,
    function_name: &str,
    window_in_seconds: i64,
) -> Result<CreateEventSourceMappingRequest> {
    let client = KinesisClient::new(Region::default());
    let output = client
        .describe_stream(DescribeStreamInput {
            stream_name: stream_name.to_string(),
            ..DescribeStreamInput::default()
        })
        .await
        .unwrap();

    Ok(CreateEventSourceMappingRequest {
        // The maximum number of items to retrieve in a single batch.
        // Amazon Kinesis - Default 100. Max 10,000.
        batch_size: Some(10000),
        // If true, the event source mapping is active. Set to false to pause polling and
        // invocation.
        enabled: Some(true),
        // The Amazon Resource Name (ARN) of the event source.
        // Amazon Kinesis - The ARN of the data stream or a stream consumer.
        event_source_arn: Some(output.stream_description.stream_arn),
        // The name of the Lambda function.
        function_name: function_name.to_owned(),
        // The maximum amount of time to gather records before invoking the function, in seconds.
        maximum_batching_window_in_seconds: Some(300),
        // The number of batches to process from each shard concurrently.
        // The parallelization factor can be scaled up to 10.
        // <https://aws.amazon.com/blogs/compute/new-aws-lambda-scaling-controls-for-kinesis-and-dynamodb-event-sources>
        parallelization_factor: Some(4),
        // The position in a stream from which to start reading. Required for Amazon Kinesis, Amazon
        // DynamoDB, and Amazon MSK Streams sources.
        starting_position: Some("LATEST".to_owned()),
        // The duration of a processing window in seconds. The range is between 1 second up to 15
        // minutes.
        tumbling_window_in_seconds: Some(window_in_seconds),
        ..CreateEventSourceMappingRequest::default()
    })
}

/// Converts Kinesis event to record batch in Arrow.
pub fn to_batch(event: KinesisEvent) -> Vec<RecordBatch> {
    // infer schema based on the first record
    let record: &[u8] = &event.records[0].kinesis.data.0.clone();
    let mut reader = BufReader::new(record);
    let schema = Arc::new(infer_json_schema(&mut reader, Some(1)).unwrap());

    // The default batch size when using the
    // [`ReaderBuilder`](json::Reader::ReaderBuilder) is 1024 records
    let batch_size = 1024;
    let input: &[u8] = &event
        .records
        .into_par_iter()
        .flat_map(|r| {
            r.kinesis
                .data
                .0
                .into_iter()
                .chain(vec![10].into_iter())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    // transform data to record batch in Arrow
    reader = BufReader::with_capacity(input.len(), input);
    let mut reader = json::Reader::from_buf_reader(reader, schema, batch_size, None);

    let mut batches = vec![];
    while let Some(batch) = reader.next().unwrap() {
        batches.push(batch);
    }
    batches
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    #[ignore]
    fn example_kinesis_event() {
        let data = include_bytes!("../tests/data/example-kinesis-event.json");
        let parsed: KinesisEvent = serde_json::from_slice(data).unwrap();
        let output: String = serde_json::to_string(&parsed).unwrap();
        let reparsed: KinesisEvent = serde_json::from_slice(output.as_bytes()).unwrap();
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn example_reader() {
        let records: &[u8] = include_str!("../tests/data/mixed_arrays.txt").as_bytes();
        let mut reader = BufReader::new(records);
        let schema = Arc::new(infer_json_schema(&mut reader, Some(1)).unwrap());

        let batch_size = 1024;
        let mut reader =
            json::Reader::new(BufReader::new(records), schema.clone(), batch_size, None);
        let batch = reader.next().unwrap().unwrap();
        assert_eq!(4, batch.num_rows());
        assert_eq!(4, batch.num_columns());

        let batch_size = 4;
        let mut reader =
            json::Reader::new(BufReader::new(records), schema.clone(), batch_size, None);
        let batch = reader.next().unwrap().unwrap();
        assert_eq!(4, batch.num_rows());
        assert_eq!(4, batch.num_columns());

        let batch_size = 2;
        let mut reader =
            json::Reader::new(BufReader::new(records), schema.clone(), batch_size, None);
        let batch = reader.next().unwrap().unwrap();
        assert_eq!(2, batch.num_rows());
        assert_eq!(4, batch.num_columns());

        let batch_size = 1;
        let mut reader =
            json::Reader::new(BufReader::new(records), schema.clone(), batch_size, None);
        let batch = reader.next().unwrap().unwrap();
        assert_eq!(1, batch.num_rows());
        assert_eq!(4, batch.num_columns());

        let batch_size = 3;
        let mut reader = json::Reader::new(BufReader::new(records), schema, batch_size, None);
        let batch = reader.next().unwrap().unwrap();
        assert_eq!(3, batch.num_rows());
        assert_eq!(4, batch.num_columns());
        let batch = reader.next().unwrap().unwrap();
        assert_eq!(1, batch.num_rows());
        assert_eq!(4, batch.num_columns());
    }
}
