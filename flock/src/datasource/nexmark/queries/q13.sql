-- -------------------------------------------------------------------------------------------------
-- Query 13: Bounded Side Input Join (Not in original suite)
-- -------------------------------------------------------------------------------------------------
-- Joins a stream to a bounded side input, modeling basic stream enrichment.
-- -------------------------------------------------------------------------------------------------

-- https://nightlies.apache.org/flink/flink-docs-release-1.11/dev/table/streaming/temporal_tables.html
-- https://nightlies.apache.org/flink/flink-docs-release-1.11/dev/table/streaming/joins.html#join-with-a-temporal-table

CREATE TABLE side_input (
  key BIGINT,
  `value` VARCHAR
) WITH (
  'connector.type' = 'filesystem',
  'connector.path' = 'file://${FLINK_HOME}/data/side_input.txt',
  'format.type' = 'csv'
);

SELECT
    B.auction,
    B.bidder,
    B.price,
    B.dateTime,
    S.`value`
FROM (SELECT *, PROCTIME() as p_time FROM bid) B
JOIN side_input FOR SYSTEM_TIME AS OF B.p_time AS S
ON mod(B.auction, 10000) = S.key;
