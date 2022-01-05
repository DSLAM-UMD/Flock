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

//! This module contains the encoding and decoding functions to be used in the
//! cloud function runtime.
//!
//! For example, it can be used to reduce the size of the payload sent between
//! the cloud functions, and reduce the size of all environment variables to
//! less than 4KB as well.

use super::error::{FlockError, Result};
use lz4::block::CompressionMode;
use serde::{Deserialize, Serialize};

/// This function encodes the given data into a byte array.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub enum Encoding {
    /// Snappy is a LZ77-type compressor with a fixed, byte-oriented encoding.
    /// It does not aim for maximum compression, or compatibility with any other
    /// compression library; instead, it aims for very high speeds and
    /// reasonable compression.
    /// <https://github.com/burntsushi/rust-snappy>
    Snappy,
    /// LZ4 is a very fast lossless compression algorithm, providing compression
    /// speed at 400 MB/s per core, with near-linear scalability for
    /// multi-threaded applications. It also features an extremely fast decoder,
    /// with speed in multiple GB/s per core, typically reaching RAM speed
    /// limits on multi-core systems.
    /// <https://github.com/bozaro/lz4-rs>
    Lz4,
    /// A streaming compression/decompression library DEFLATE-based streams.
    /// <https://github.com/rust-lang/flate2-rs>
    Zlib,
    /// A fast lossless compression algorithm, targeting real-time compression
    /// scenarios at zlib-level and better compression ratios. <https://github.com/facebook/zstd>
    Zstd,
    /// No compression/decompression applied to the context.
    None,
}

impl Default for Encoding {
    fn default() -> Encoding {
        Encoding::Zstd
    }
}

impl Encoding {
    /// Compress the given data using the encoding type.
    pub fn compress(&self, s: &[u8]) -> Result<Vec<u8>> {
        Ok(match *self {
            Encoding::Snappy => {
                let mut encoder = snap::raw::Encoder::new();
                encoder
                    .compress_vec(s)
                    .map_err(|e| FlockError::Execution(e.to_string()))?
            }
            Encoding::Lz4 => {
                // TODO(gangliao): more flexible way to set the compression level
                lz4::block::compress(s, Some(CompressionMode::HIGHCOMPRESSION(6)), true)
                    .map_err(|e| FlockError::Execution(e.to_string()))?
            }
            Encoding::Zstd => {
                zstd::block::compress(s, 3).map_err(|e| FlockError::Execution(e.to_string()))?
            }
            Encoding::None => s.into(),
            _ => unimplemented!(),
        })
    }

    /// Decompress the given data using the encoding type.
    pub fn decompress(&self, s: &[u8]) -> Result<Vec<u8>> {
        Ok(match *self {
            Encoding::Snappy => {
                let mut decoder = snap::raw::Decoder::new();
                decoder
                    .decompress_vec(s)
                    .map_err(|e| FlockError::Execution(e.to_string()))?
            }
            Encoding::Lz4 => {
                lz4::block::decompress(s, None).map_err(|e| FlockError::Execution(e.to_string()))?
            }
            Encoding::Zstd => zstd::block::decompress(s, 10485760)
                .map_err(|e| FlockError::Execution(e.to_string()))?,
            Encoding::None => s.into(),
            _ => {
                unimplemented!();
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::arrow::array::*;
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;

    use datafusion::datasource::MemTable;
    use datafusion::execution::context::ExecutionContext;
    use std::sync::Arc;
    use std::time::Instant;

    #[tokio::test]
    async fn encode_plan() -> Result<()> {
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

        let mut ctx = ExecutionContext::new();

        let table1 = MemTable::try_new(schema1, vec![vec![batch1]])?;
        let table2 = MemTable::try_new(schema2, vec![vec![batch2]])?;

        ctx.register_table("t1", Arc::new(table1))?;
        ctx.register_table("t2", Arc::new(table2))?;

        let sql = concat!(
            "SELECT a, b, d ",
            "FROM t1 JOIN t2 ON a = c ",
            "ORDER BY b ASC ",
            "LIMIT 3"
        );

        let plan = ctx.create_logical_plan(sql)?;
        let plan = ctx.optimize(&plan)?;
        let plan = ctx.create_physical_plan(&plan).await?;

        for en in [Encoding::Snappy, Encoding::Lz4, Encoding::Zstd].iter() {
            let json = serde_json::to_string(&plan).unwrap();

            let now = Instant::now();
            let en_json = en.compress(json.as_bytes())?;
            println!("Compression time: {} μs", now.elapsed().as_micros());

            let now = Instant::now();
            let de_json = en.decompress(&en_json)?;
            println!("Decompression time: {} μs", now.elapsed().as_micros());

            println!(
                concat!(
                    "Plan: before compression: {} bytes, ",
                    "after compression: {} bytes, ",
                    "type: {:?}, ratio: {:.3}"
                ),
                json.len(),
                en_json.len(),
                en,
                json.len() as f64 / en_json.len() as f64
            );

            assert_eq!(json, unsafe { std::str::from_utf8_unchecked(&de_json) });
        }

        Ok(())
    }
}
