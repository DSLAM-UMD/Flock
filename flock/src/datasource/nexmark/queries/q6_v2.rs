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

#[allow(dead_code)]
fn main() {}

#[cfg(test)]
mod tests {
    use crate::datasource::epoch::Epoch;
    use crate::datasource::nexmark::event::{Auction, Bid};
    use crate::datasource::nexmark::NEXMarkSource;
    use crate::error::Result;
    use crate::runtime::plan::physical_plan;
    use crate::stream::Window;
    use crate::transmute::event_bytes_to_batch;
    use datafusion::arrow::util::pretty::pretty_format_batches;
    use datafusion::datasource::MemTable;
    use datafusion::physical_plan::collect;
    use indoc::indoc;
    use std::sync::Arc;

    #[tokio::test]
    async fn local_query_6_v2() -> Result<()> {
        // benchmark configuration
        let seconds = 2;
        let threads = 1;
        let event_per_second = 1000;
        let nex = NEXMarkSource::new(seconds, threads, event_per_second, Window::ElementWise);

        // data source generation
        let events = nex.generate_data()?;

        let sql = indoc! {"
            SELECT seller,
                   Avg(final)
            FROM   (SELECT ROW_NUMBER()
                            OVER (
                                PARTITION BY seller
                                ORDER BY date_time DESC) AS row,
                            seller,
                            final
                    FROM   (SELECT  seller,
                                    Max(price)       AS final,
                                    Max(b_date_time) AS date_time
                            FROM    auction
                                    INNER JOIN bid
                                            ON a_id = auction
                            WHERE  b_date_time BETWEEN a_date_time AND expires
                            GROUP  BY a_id,
                                    seller) AS Q) AS R
            WHERE  row <= 10
            GROUP  BY seller;
        "};

        let auction_schema = Arc::new(Auction::schema());
        let bid_schema = Arc::new(Bid::schema());

        // sequential processing
        for i in 0..seconds {
            // events to record batches
            let am = events.auctions.get(&Epoch::new(i)).unwrap();
            let (auctions, _) = am.get(&0).unwrap();
            let auctions_batches = event_bytes_to_batch(auctions, auction_schema.clone(), 1024);

            let bm = events.bids.get(&Epoch::new(i)).unwrap();
            let (bids, _) = bm.get(&0).unwrap();
            let bids_batches = event_bytes_to_batch(bids, bid_schema.clone(), 1024);

            // register memory tables
            let mut ctx = datafusion::execution::context::ExecutionContext::new();
            let auction_table = MemTable::try_new(auction_schema.clone(), vec![auctions_batches])?;
            ctx.deregister_table("auction")?;
            ctx.register_table("auction", Arc::new(auction_table))?;

            let bid_table = MemTable::try_new(bid_schema.clone(), vec![bids_batches])?;
            ctx.deregister_table("bid")?;
            ctx.register_table("bid", Arc::new(bid_table))?;

            // optimize query plan and execute it
            let plan = physical_plan(&ctx, sql).await?;
            let output_partitions = collect(plan).await?;

            // show output
            println!("{}", pretty_format_batches(&output_partitions)?);
        }

        Ok(())
    }
}
