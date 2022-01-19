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

//! This crate contains all wrapped functions of the AWS S3 service.

use crate::configs::*;
use crate::error::{FlockError, Result};
use rayon::prelude::*;
use rusoto_core::ByteStream;
use rusoto_s3::{
    CreateBucketRequest, GetObjectRequest, HeadBucketRequest, ListObjectsV2Request,
    PutObjectRequest, S3,
};
use std::io::Read;

/// Puts an object to AWS S3 if the object does not exist. If the object exists,
/// it isn't modified.
///
/// # Arguments
/// * `bucket` - The name of the bucket to put the object in.
/// * `key` - The key of the object to put.
/// * `body` - The body of the object to put.
pub async fn put_object_if_missing(bucket: &str, key: &str, body: Vec<u8>) -> Result<()> {
    if let Some(0) = FLOCK_S3_CLIENT
        .list_objects_v2(ListObjectsV2Request {
            bucket: bucket.to_owned(),
            prefix: Some(key.to_owned()),
            max_keys: Some(1),
            ..Default::default()
        })
        .await
        .map_err(|e| FlockError::Internal(e.to_string()))?
        .key_count
    {
        FLOCK_S3_CLIENT
            .put_object(PutObjectRequest {
                bucket: bucket.to_owned(),
                key: key.to_owned(),
                body: Some(ByteStream::from(body)),
                ..Default::default()
            })
            .await
            .map_err(|e| FlockError::AWS(e.to_string()))?;
    }
    Ok(())
}

/// Puts an object to AWS S3. If the object exists, it is overwritten.
///
/// # Arguments
/// * `bucket` - The name of the bucket to put the object in.
/// * `key` - The key of the object to put.
/// * `body` - The body of the object to put.
pub async fn put_object(bucket: &str, key: &str, body: Vec<u8>) -> Result<()> {
    FLOCK_S3_CLIENT
        .put_object(PutObjectRequest {
            bucket: bucket.to_owned(),
            key: key.to_owned(),
            body: Some(ByteStream::from(body)),
            ..Default::default()
        })
        .await
        .map_err(|e| FlockError::AWS(e.to_string()))
        .map(|_| ())
}

/// Puts an object to AWS S3. If the object exists, it is overwritten.
///
/// # Arguments
/// * `bucket` - The name of the bucket to put the object in.
/// * `key` - The key of the object to put.
/// * `body` - The body of the object to put.
/// * `content_type` - The content type of the object to put.
///   * `application/octet-stream` - The default content type. This is used to
///     support OCR and Parquet.
///   * `application/json` - The content type for JSON.
///   * `text/plain` - The content type for plain text.
///   * `text/csv` - The content type for CSV.
///   * `text/html` - The content type for HTML.
///   * `text/xml` - The content type for XML.
///   * `image/jpeg` - The content type for JPEG.
pub async fn put_object_with_content_type(
    bucket: &str,
    key: &str,
    body: Vec<u8>,
    content_type: &str,
) -> Result<()> {
    FLOCK_S3_CLIENT
        .put_object(PutObjectRequest {
            bucket: bucket.to_owned(),
            key: key.to_owned(),
            body: Some(ByteStream::from(body)),
            content_type: Some(content_type.to_owned()),
            ..Default::default()
        })
        .await
        .map_err(|e| FlockError::AWS(e.to_string()))
        .map(|_| ())
}

/// Gets object from AWS S3.
/// If the object does not exist, it returns an empty body.
///
/// # Arguments
/// * `bucket` - The name of the bucket to get the object from.
/// * `key` - The key of the object to get.
///
/// # Returns
/// The body of the object.
pub async fn get_object(bucket: &str, key: &str) -> Result<Vec<u8>> {
    let body = FLOCK_S3_CLIENT
        .get_object(GetObjectRequest {
            bucket: bucket.to_owned(),
            key: key.to_owned(),
            ..Default::default()
        })
        .await
        .unwrap()
        .body
        .take()
        .expect("body is empty");

    Ok(tokio::task::spawn_blocking(move || {
        let mut buf = Vec::new();
        body.into_blocking_read().read_to_end(&mut buf).unwrap();
        buf
    })
    .await
    .expect("failed to load plan from S3"))
}

/// Checks if a bucket exists in AWS S3.
///
/// # Arguments
/// * `bucket` - The name of the bucket to check.
pub async fn bucket_exists(bucket: &str) -> Result<bool> {
    // Returns a list of all buckets owned by the authenticated sender of the
    // request.
    let resp = FLOCK_S3_CLIENT
        .list_buckets()
        .await
        .map_err(|e| FlockError::AWS(e.to_string()))?;

    Ok(resp
        .buckets
        .par_iter()
        .flatten()
        .any(|b| b.name == Some(bucket.to_owned())))
}

/// Checks whether the specified bucket exists in Amazon S3 and you have
/// permission to access it.
///
/// # Arguments
/// * `bucket` - The name of the bucket to check.
pub async fn bucket_exists_and_accessible(bucket: &str) -> Result<bool> {
    // This action is useful to determine if a bucket exists and you have permission
    // to access it. The action returns a 200 OK if the bucket exists and you have
    // permission to access it.
    //
    // If the bucket does not exist or you do not have permission to access it, the
    // HEAD request returns a generic 404 Not Found or 403 Forbidden code. A message
    // body is not included, so you cannot determine the exception beyond these
    // error codes.
    match FLOCK_S3_CLIENT
        .head_bucket(HeadBucketRequest {
            bucket: bucket.to_owned(),
            ..Default::default()
        })
        .await
        .map_err(|e| FlockError::AWS(e.to_string()))
    {
        Ok(_) => Ok(true),
        Err(e) => Ok(false),
    }
}

/// Creates a new S3 bucket. To create a bucket, you must register with Amazon
/// S3 and have a valid AWS Access Key ID to authenticate requests. Anonymous
/// requests are never allowed to create buckets. By creating the bucket, you
/// become the bucket owner.
///
/// Not every string is an acceptable bucket name. For information about bucket
/// naming restrictions.
///
/// By default, the bucket is created in the US East (N. Virginia) Region. You
/// can optionally specify a Region in the request body. You might choose a
/// Region to optimize latency, minimize costs, or address regulatory
/// requirements. For example, if you reside in Europe, you will probably find
/// it advantageous to create buckets in the Europe (Ireland) Region.
pub async fn create_bucket_if_missing(bucket: &str) -> Result<()> {
    if !bucket_exists_and_accessible(bucket).await? {
        FLOCK_S3_CLIENT
            .create_bucket(CreateBucketRequest {
                bucket: bucket.to_owned(),
                ..Default::default()
            })
            .await
            .map_err(|e| FlockError::AWS(e.to_string()))?;
    }
    Ok(())
}
