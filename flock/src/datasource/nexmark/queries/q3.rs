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
    use crate::datasource::nexmark::event::{Auction, Person};
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
    async fn local_query_3() -> Result<()> {
        // benchmark configuration
        let seconds = 5;
        let threads = 1;
        let event_per_second = 1000;
        let nex = NEXMarkSource::new(seconds, threads, event_per_second, Window::ElementWise);

        // data source generation
        let events = nex.generate_data()?;

        let sql = indoc! {"
            SELECT name,
                   city,
                   state,
                   a_id
            FROM   auction
                   INNER JOIN person
                           ON seller = p_id
            WHERE  category = 10
                   AND ( state = 'or'
                        OR state = 'id'
                        OR state = 'ca' );
        "};

        let auction_schema = Arc::new(Auction::schema());
        let person_schema = Arc::new(Person::schema());

        // sequential processing
        for i in 0..seconds {
            // events to record batches
            let am = events.auctions.get(&Epoch::new(i)).unwrap();
            let (auctions, _) = am.get(&0).unwrap();
            let auctions_batches = event_bytes_to_batch(auctions, auction_schema.clone(), 1024);

            let pm = events.persons.get(&Epoch::new(i)).unwrap();
            let (persons, _) = pm.get(&0).unwrap();
            let person_batches = event_bytes_to_batch(persons, person_schema.clone(), 1024);

            // register memory tables
            let mut ctx = datafusion::execution::context::ExecutionContext::new();
            let auction_table = MemTable::try_new(auction_schema.clone(), vec![auctions_batches])?;
            ctx.deregister_table("auction")?;
            ctx.register_table("auction", Arc::new(auction_table))?;

            let person_table = MemTable::try_new(person_schema.clone(), vec![person_batches])?;
            ctx.deregister_table("person")?;
            ctx.register_table("person", Arc::new(person_table))?;

            // optimize query plan and execute it
            let physical_plan = physical_plan(&ctx, sql).await?;
            let batches = collect(physical_plan).await?;

            // show output
            println!("{}", pretty_format_batches(&batches)?);
        }

        Ok(())
    }
}
