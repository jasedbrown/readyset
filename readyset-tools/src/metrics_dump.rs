#![warn(clippy::panic)]
//! Tool to retrieve a metrics dump for the current leader in a
//! deployment.

use clap::builder::NonEmptyStringValueParser;
use clap::Parser;
use readyset_client::consensus::AuthorityType;
use readyset_client::metrics::client::MetricsClient;
use readyset_client::ReadySetHandle;

#[derive(Parser)]
#[clap(name = "metrics_dump")]
struct MetricsDump {
    #[clap(short, long, env("AUTHORITY_ADDRESS"), default_value("127.0.0.1:2181"))]
    authority_address: String,

    #[clap(long, env("AUTHORITY"), default_value("zookeeper"), value_parser = ["consul", "zookeeper"])]
    authority: AuthorityType,

    #[clap(short, long, env("DEPLOYMENT"), value_parser = NonEmptyStringValueParser::new())]
    deployment: String,
}

impl MetricsDump {
    pub async fn run(self) -> anyhow::Result<()> {
        let authority = self
            .authority
            .to_authority(&self.authority_address, &self.deployment)
            .await;

        let mut handle: ReadySetHandle = ReadySetHandle::new(authority).await;
        handle.ready().await.unwrap();

        let mut client = MetricsClient::new(handle).unwrap();
        let res = client.get_metrics().await?;
        println!("{:?}", res);

        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let metrics_dump = MetricsDump::parse();
    metrics_dump.run().await
}
