use std::collections::HashMap;

use std::fs::File;
use std::io::Read;
use std::vec::Vec;
use std::error::Error as StdError;
use std::pin::Pin;
use std::future::Future;
use std::time::Duration;
use std::{env, process::exit};

use serde::{Deserialize, Serialize};
use serde_json;

use log::{info, debug};

use reqwest::Url;
use reqwest;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};

use rust_decimal::Decimal;

use thousands::{Separable, SeparatorPolicy, digits};

use matrix_sdk::{
    config::SyncSettings,
    ruma::events::room::{
        member::StrippedRoomMemberEvent,
        message::{MessageType, OriginalSyncRoomMessageEvent, RoomMessageEventContent},
    },
    Client as MatrixClient, Room, RoomState,
};

use indoc::formatdoc;

use tokio::{task, time};
use tokio_postgres::{Client, Error, NoTls};
use tokio_postgres::types::Type;

use futures::stream::FuturesUnordered;
use futures::StreamExt;







struct Database {
    client: Client,
}

struct Matrix {
    
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
enum SqlValue {
    Int(i32),
    Bigint(i64),
    Text(String),
    // Date(NaiveDate),
    Numeric(Decimal),
    // Add more types here as needed.
}

#[derive(Debug, Deserialize)]
struct DatabaseConfig {
    hostaddr: String,
    port: u16,
    user: String,
    password: String,
    dbname: String,
    matrixuser: String,
    matrixpassword: String,
    matrixroom: String,
    matrixtoken: String,
    matrixhomeserver: String,
    slotschedule: u16,
}


#[derive(Serialize, Deserialize, Debug)]
struct MatrixAuth {
    user_id: String,
    access_token: String,
    home_server: String,
    device_id: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct Blocks {
    epoch_no: i64,
    blocks_forged: i64,
}

#[derive(Serialize, Deserialize, Debug)]
struct Delegator {
    addr_view: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct Address {
    stake_address: String,
    ada_value: Decimal,
    from_pool: String,
    to_pool: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct PoolStake {
    live_stake: Decimal,
}

#[derive(Serialize, Deserialize, Debug)]
struct PoolStats {
    live_stake: Decimal,
    live_saturation: Decimal,
    live_delegator_count: i64,
}






#[tokio::main]
async fn main() -> Result<(), Box<dyn StdError>>  {

    tokio::task::spawn(async {

            let matrix_homeserver = env::var("MATRIX_HOMESERVER").expect("Error: MATRIX_HOMESERVER not found");
            let matrix_user = env::var("MATRIX_USER").expect("Error: MATRIX_USER not found");
            let matrix_password = env::var("MATRIX_PASSWORD").expect("Error: MATRIX_PASSWORD not found");
            

            // tracing_subscriber::fmt::init();
            
            Matrix::login_and_sync(matrix_homeserver, &matrix_user, &matrix_password).await?;
            Ok::<(), anyhow::Error>(())


    });


    //env::set_var("RUST_BACKTRACE", "full");

    //env_logger::init();

    let mut prevforged: Vec<HashMap<String, i64>> = vec![];
    let mut prevdelegators: Vec<HashMap<String, String>> = vec![];
    let mut prevpoolstake: Vec<HashMap<String, Decimal>> = vec![];

    let mut interval = time::interval(Duration::from_secs(60));


        loop {
            interval.tick().await;

            let mut tasks = FuturesUnordered::<Pin<Box<dyn Future<Output = String>>>>::new();
            
            tasks.push(Box::pin(blocks(&mut prevforged)));
            tasks.push(Box::pin(delegators(&mut prevdelegators)));
            tasks.push(Box::pin(stake(&mut prevpoolstake)));

            while let Some(result) = tasks.next().await {
                println!("{}", result);

            }
        }
    }



    

    async fn blocks(prevforged: &mut Vec<HashMap<String, i64>>) -> String {
        let db = Database::new().await.expect("Problem with db connection");
        db.ping().await;
    
        let curforged = db.fetch_block_data("Select * from balance.bot_blocks_forged").await.expect("Problem with pull latest block forge data");

        if prevforged.is_empty() {
            *prevforged = curforged.clone();
            println!("Startup block forge data loaded");

        } else {
            let slots_assigned = env::var("SLOTS_ASSIGNED").expect("Error: SLOTS_ASSIGNED not found");
            println!("Checking for block forge updates {}", slots_assigned);
            let blockdiff: Vec<_> = curforged.into_iter().filter(|item| !prevforged.contains(item)).collect();

            if blockdiff.is_empty() {
                        println!(" -- No new blocks forged");
                        
                    } else {
                        println!(" -- New blocks forged!  Sending message....");
        
                        
                        let slots_assigned = env::var("SLOTS_ASSIGNED").expect("Error: SLOTS_ASSIGNED not found");
        
                        let map =  serde_json::to_string(&blockdiff[0]).unwrap();
                        let p: Blocks = serde_json::from_str(&map).expect("REASON");
                        
                        let blockmsg: String = format!("⚒️   {} / {}  blocks forged for epoch  {}", p.blocks_forged, &slots_assigned, p.epoch_no);
                       
        
                        Matrix::message(&blockmsg).await.expect("REASON");
                        
                        
                        *prevforged = blockdiff.clone();
                        
                    }
        }
        "Task - Forged Blocks Complete".to_owned()
    }

    async fn delegators(prevdelegators: &mut Vec<HashMap<String, String>>) -> String {
        let db = Database::new().await.expect("Problem with db connection");
        db.ping().await;

        let curdelegators = db.fetch_delegator_data("Select * from balance.bot_delegator_list").await.expect("Problem with pulling latest delegator data");

            if prevdelegators.is_empty() {
                *prevdelegators = curdelegators.clone();
                println!("Startup delegator data loaded");
            
            } else {
                println!("Checking for delegator updates");
    
                let arrivals: Vec<_> = curdelegators.clone().into_iter().filter(|item| !prevdelegators.contains(item)).collect();
                let departures: Vec<_> = prevdelegators.clone().into_iter().filter(|item| !curdelegators.contains(item)).collect();
    
                if departures.is_empty() {
                    println!(" -- No departures found");
    
                } else {
    
                    println!(" -- New departures found....processing");
    
                    for (i, row_map) in departures.iter().enumerate() {
    
                        for (key, value) in row_map.iter() {
    
                            let addressquery: String = format!("Select * From balance.bot_address_value('{}')", value);
    
                            let departuredata = db.fetch_address_data(&addressquery).await.expect("REASON");
    
                            let serialized = serde_json::to_string(&departuredata).unwrap();;
                            let deserialized: Vec<Address> = serde_json::from_str(&serialized).unwrap();
    
                            let policy = SeparatorPolicy {
                                separator: ',',
                                groups:    &[3],
                                digits:    digits::ASCII_DECIMAL,
                            };
    
                            let ada = deserialized[0].ada_value.separate_by_policy(policy);
                            let address = &deserialized[0].stake_address[..10];
                            let newpool = &deserialized[0].to_pool;
    
                            let departuresmsg: String = formatdoc!(r#"
                            ❌   {} ₳  Delegation Departing   🙏
                                ▫️  Stake Address  {}
                                ▫️  To  {}"#, ada, address, newpool);
    
                            Matrix::message(&departuresmsg).await.expect("REASON");
    
                            *prevdelegators = curdelegators.clone();
    
                            println!(" -- New departures send message complete");
    
                        }
                    }
    
                }
    
                if arrivals.is_empty() {
                    println!(" -- No arrivals found");
    
                } else {
    
                    println!(" -- New arrivals found....processing");
    
                    for (i, row_map) in arrivals.iter().enumerate() {
    
                        for (key, value) in row_map.iter() {
    
                            let addressquery: String = format!("Select * From balance.bot_address_value('{}')", value);
    
                            let arrivaldata = db.fetch_address_data(&addressquery).await.expect("REASON");
    
                            let serialized = serde_json::to_string(&arrivaldata).unwrap();;
                            let deserialized: Vec<Address> = serde_json::from_str(&serialized).unwrap();
    
                            let policy = SeparatorPolicy {
                                separator: ',',
                                groups:    &[3],
                                digits:    digits::ASCII_DECIMAL,
                            };
    
                            let ada = deserialized[0].ada_value.separate_by_policy(policy);
                            let address = &deserialized[0].stake_address[..10];
                            let prevpool = &deserialized[0].from_pool;
    
                            if prevpool != "" {
                                let arrivalsmsg: String = formatdoc!(r#"
                                ✅   {} ₳  Delegation Arriving   👏 
                                    ▫️  Stake Address  {}
                                    ▫️  From  {}"#, ada, address, prevpool);
    
                                Matrix::message(&arrivalsmsg).await.expect("REASON");
    
                                *prevdelegators = curdelegators.clone();
    
                                println!(" -- New arrivals send message complete");
    
                            } else {
                                let arrivalsmsg: String = formatdoc!(r#"
                                ✅   {} ₳  Delegation Arriving   👏 
                                    ▫️  Stake Address  {}"#, ada, address);
    
                                Matrix::message(&arrivalsmsg).await.expect("REASON");
    
                                *prevdelegators = curdelegators.clone();
    
                                println!(" -- New arrivals send message complete");
    
    
                            }
                        }
                    }
                }
            }

        "Task - Delegators Complete".to_owned()
    }

    async fn stake(prevpoolstake: &mut Vec<HashMap<String, Decimal>>) -> String {
        let db = Database::new().await.expect("Problem with db connection");
        db.ping().await;

        let curpoolstake = db.fetch_poolstake_data("Select * From balance.bot_live_stake").await.expect("REASON");

        if prevpoolstake.is_empty() {
            *prevpoolstake = curpoolstake.clone();
            println!("Startup pool stake data loaded");
            
        } else {
            println!("Checking for pool stake updates");

            let stakediff: Vec<_> = curpoolstake.clone().into_iter().filter(|item| !prevpoolstake.contains(item)).collect();

            let curstakeserialized2 = serde_json::to_string(&curpoolstake).unwrap();
            let curstakedeserialized2: Vec<PoolStake> = serde_json::from_str(&curstakeserialized2).unwrap();
            

            if stakediff.is_empty() {
                println!(" -- No live stake updates found");

            } else {

                println!(" -- Pool stake updates found....processing");

                let prevstakeserialized = serde_json::to_string(&prevpoolstake).unwrap();
                let prevstakedeserialized: Vec<PoolStake> = serde_json::from_str(&prevstakeserialized).unwrap();
                let curstakeserialized = serde_json::to_string(&curpoolstake).unwrap();
                let curstakedeserialized: Vec<PoolStake> = serde_json::from_str(&curstakeserialized).unwrap();
        
                let diff = curstakedeserialized[0].live_stake - prevstakedeserialized[0].live_stake;
                let negativebuffer = Decimal::new(-10000000, 2);
                let positivebuffer = Decimal::new(10000000, 2);

                let policy = SeparatorPolicy {
                    separator: ',',
                    groups:    &[3],
                    digits:    digits::ASCII_DECIMAL,
                };

                if diff < negativebuffer  {

                    let value = diff.separate_by_policy(policy);
                    let totalvalue = curstakedeserialized[0].live_stake.separate_by_policy(policy);
                    let stakemsg: String = format!("❌   Live Stake   ⬇️   {} ₳", value);
                    
                    Matrix::message(&stakemsg).await.expect("REASON");
                    
                    *prevpoolstake = curpoolstake.clone();

                } else if diff > positivebuffer {

                    let value = diff.separate_by_policy(policy);
                    let totalvalue = curstakedeserialized[0].live_stake.separate_by_policy(policy);
                    let stakemsg: String = format!("✅   Live Stake   ⬆️   {} ₳", value);

                    Matrix::message(&stakemsg).await.expect("REASON");
                    
                    *prevpoolstake = curpoolstake.clone();

                } else {
                    println!(" -- Pool stake delta inside buffer....waiting");

                }
            }
        }
        "Task - Pool Stake Complete".to_owned()
    }



impl Database {

    async fn new() -> Result<Self, Error> {

        let host_address = env::var("HOST_ADDRESS").expect("Error: HOST_ADDRESS not found");
        let port = env::var("PORT").expect("Error: PORT not found");
        let user = env::var("USER").expect("Error: USER not found");
        let password = env::var("PASSWORD").expect("Error: PASSWORD not found");
        let db_name = env::var("DB_NAME").expect("Error: DB_NAME not found");
        

        let connect_params = format!(
            "host={} port={} user={} password={} dbname={}",
            host_address, port, user, password, db_name
        );

        info!("Creating a new database instance");
        let (client, connection) = tokio_postgres::connect(
            &connect_params, NoTls).await?;

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {}", e);
            }
        });

        Ok(Self { client })
    }

    async fn ping(&self) -> Result<(), Error> {
        match self.client.simple_query("SELECT 1;").await {
            Ok(_) => {
                //let timestamp = Utc::now();
                println!("Successfully connected to the database.");
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    async fn fetch_block_data(&self, query: &str) -> Result<Vec<HashMap<String, i64>>, Box<dyn std::error::Error>> {
        
        let rows = self.client.query(query, &[]).await?;

        let mut data: Vec<HashMap<String, i64>> = Vec::new();

        debug!("Query returned {} rows", rows.len());
        for row in rows.iter() {
            let mut row_map = HashMap::new();
            for column in row.columns() {
                let column_name = column.name();
                let type_ = column.type_();

                let value = if *type_ == Type::INT4 {
                    row.get(column_name)
                // } else if *type_ == Type::TEXT || *type_ == Type::VARCHAR {
                //     // Handle TEXT and VARCHAR types
                //     SqlValue::Text(row.get(column_name))
                // } else if *type_ == Type::DATE {
                //     // Handle DATE type
                //     SqlValue::Date(row.get(column_name))
                } else if *type_ == Type::INT8 {
                    row.get(column_name)
                } else {
                    // Handle other types or return an error.
                    continue;
                };
                row_map.insert(column_name.to_string(), value);
            }
            data.push(row_map);
        }
        
        Ok(data)
    }

    async fn fetch_delegator_data(&self, query: &str) -> Result<Vec<HashMap<String, String>>, Box<dyn std::error::Error>> {
        
        let rows = self.client.query(query, &[]).await?;

        let mut data: Vec<HashMap<String, String>> = Vec::new();

        debug!("Query returned {} rows", rows.len());
        for row in rows.iter() {
            let mut row_map = HashMap::new();
            for column in row.columns() {
                let column_name = column.name();
                let type_ = column.type_();

                let value = if *type_ == Type::INT4 {
                    row.get(column_name)
                } else if *type_ == Type::TEXT || *type_ == Type::VARCHAR {
                    // Handle TEXT and VARCHAR types
                    row.get(column_name)
                // } else if *type_ == Type::DATE {
                //     // Handle DATE type
                //     SqlValue::Date(row.get(column_name))
                } else if *type_ == Type::INT8 {
                    row.get(column_name)
                } else {
                    // Handle other types or return an error.
                    continue;
                };
                row_map.insert(column_name.to_string(), value);
            }
            data.push(row_map);
        }
        
        Ok(data)
    }

    async fn fetch_address_data(&self, query: &str) -> Result<Vec<HashMap<String, SqlValue>>, Box<dyn std::error::Error>> {
        
        let rows = self.client.query(query, &[]).await?;

        let mut data: Vec<HashMap<String, SqlValue>> = Vec::new();

        debug!("Query returned {} rows", rows.len());
        for row in rows.iter() {
            let mut row_map = HashMap::new();
            for column in row.columns() {
                let column_name = column.name();
                let type_ = column.type_();

                let value = if *type_ == Type::INT4 {
                    SqlValue::Int(row.get(column_name))
                } else if *type_ == Type::TEXT || *type_ == Type::VARCHAR {
                    SqlValue::Text(row.get(column_name))
                } else if *type_ == Type::INT8 {
                    SqlValue::Bigint(row.get(column_name))
                } else if *type_ == Type::NUMERIC {
                    SqlValue::Numeric(row.get(column_name))
                } else {
                    // Handle other types or return an error.
                    continue;
                };
                row_map.insert(column_name.to_string(), value);
            }
            data.push(row_map);
        }
        
        Ok(data)
    }

    async fn fetch_poolstake_data(&self, query: &str) -> Result<Vec<HashMap<String, Decimal>>, Box<dyn std::error::Error>> {
        
        let rows = self.client.query(query, &[]).await?;

        let mut data: Vec<HashMap<String, Decimal>> = Vec::new();

        debug!("Query returned {} rows", rows.len());
        for row in rows.iter() {
            let mut row_map = HashMap::new();
            for column in row.columns() {
                let column_name = column.name();
                let type_ = column.type_();

                let value = if *type_ == Type::INT4 {
                    row.get(column_name)
                } else if *type_ == Type::TEXT || *type_ == Type::VARCHAR {
                    // Handle TEXT and VARCHAR types
                    row.get(column_name)
                // } else if *type_ == Type::DATE {
                //     // Handle DATE type
                //     SqlValue::Date(row.get(column_name))
                } else if *type_ == Type::INT8 {
                    row.get(column_name)
                } else if *type_ == Type::NUMERIC {
                    row.get(column_name)
                } else {
                    // Handle other types or return an error.
                    continue;
                };
                row_map.insert(column_name.to_string(), value);
            }
            data.push(row_map);
        }
        
        Ok(data)
    }

}



impl Matrix {

    async fn new() -> Result<MatrixAuth, Box<dyn StdError>> {

        let matrix_user = env::var("MATRIX_USER").expect("Error: MATRIX_USER not found");
        let matrix_password = env::var("MATRIX_PASSWORD").expect("Error: MATRIX_PASSWORD not found");

        let mut map = HashMap::new();
        map.insert("type", "m.login.password");
        map.insert("user", &matrix_user);
        map.insert("password", &matrix_password);
        map.insert("device_id", "balance_bot_service");

        let client = reqwest::Client::new();
        let response = client
            .post("https://matrix.forum.balanceanalytics.io/_matrix/client/r0/login")
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .json(&map)
            .send()
            .await
            .unwrap();

            let authdata: MatrixAuth = response.json::<MatrixAuth>().await?;
            println!("Matrix Access Token {}", authdata.access_token);

        Ok(authdata)
    }

    async fn message(query: &str) -> Result<(), Box<dyn StdError>> {

        let matrix_room = env::var("MATRIX_ROOM").expect("Error: MATRIX_ROOM not found");
        let matrix_token = env::var("MATRIX_TOKEN").expect("Error: MATRIX_TOKEN not found");

        let mut url: String = format!("https://matrix.forum.balanceanalytics.io/_matrix/client/r0/rooms/{}/send/m.room.message?access_token={}", matrix_room, matrix_token);

        let mut map = HashMap::new();
        map.insert("msgtype", "m.text");
        map.insert("body", query);

        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .json(&map)
            .send()
            .await
            .unwrap();

        Ok(())
    }

    async fn login_and_sync(homeserver_url: String, username: &str, password: &str) -> anyhow::Result<()> {
        
        let client = MatrixClient::builder()
            .homeserver_url(homeserver_url)
            .build()
            .await?;
    
        client
            .matrix_auth()
            .login_username(username, password)
            .initial_device_display_name("getting started bot")
            .await?;
    
        println!("logged in as {username}");
    
        let sync_token = client.sync_once(SyncSettings::default()).await.unwrap().next_batch;
    
        client.add_event_handler(Matrix::on_room_message);
    
        let settings = SyncSettings::default().token(sync_token);
        
        client.sync(settings).await?;
    
        Ok(())
    }

    async fn on_room_message(event: OriginalSyncRoomMessageEvent, room: Room) {

        if room.state() != RoomState::Joined {
            return;
        }
        let MessageType::Text(text_content) = event.content.msgtype else { return };
    
        if text_content.body.contains("!party") {
            let content = RoomMessageEventContent::text_plain("🎉🎊🥳 let's PARTY!! 🥳🎊🎉");
    
            // println!("sending");
    
            room.send(content).await.unwrap();
    
            // println!("message sent");
        }

        if text_content.body.contains("!boo") {
            let content = RoomMessageEventContent::text_plain("👻  Booooo!!  👻");
    
            // println!("sending");
    
            room.send(content).await.unwrap();
    
            // println!("message sent");
        }

        if text_content.body.contains("!status") {

            let db = Database::new().await.unwrap();

            let poolstatsquery: String = format!("Select * From balance.bot_pool_stats");

            let poolstatsdata = db.fetch_address_data(&poolstatsquery).await.unwrap();

            let serialized = serde_json::to_string(&poolstatsdata).unwrap();;
            let deserialized: Vec<PoolStats> = serde_json::from_str(&serialized).unwrap();

            let policy = SeparatorPolicy {
                separator: ',',
                groups:    &[3],
                digits:    digits::ASCII_DECIMAL,
            };

            let live_stake = deserialized[0].live_stake.separate_by_policy(policy);
            let live_saturation = &deserialized[0].live_saturation.separate_by_policy(policy);
            let live_delegator_count = &deserialized[0].live_delegator_count;

            let poolstatsmsg: String = formatdoc!(r#"
            ⚖️    BALNC Pool Statistics   🧐
                ▫️  Stake            {} ₳
                ▫️  Saturation    {} %
                ▫️  Delegates     {}"#, live_stake, live_saturation, live_delegator_count);



            let content = RoomMessageEventContent::text_plain(&poolstatsmsg);
    
            // println!("sending");
    
            room.send(content).await.unwrap();
    
            // println!("message sent");
        }
    }

}



