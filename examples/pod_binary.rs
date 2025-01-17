use futures::{StreamExt, TryStreamExt};
use k8s_openapi::api::core::v1::Pod;
use tracing::*;

use kube::{
    api::{Api, AttachParams, DeleteParams, PostParams, ResourceExt, WatchEvent, WatchParams},
    Client,
};
use tokio::io::AsyncWriteExt;

// A `kubectl cp` analog example.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let client = Client::try_default().await?;

    let p: Pod = serde_json::from_value(serde_json::json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": { "name": "example" },
        "spec": {
            "containers": [{
                "name": "example",
                "image": "ubuntu:20.04",
                // Do nothing
                "command": ["tail", "-f", "/dev/null"],
            }],
        }
    }))?;

    let pods: Api<Pod> = Api::default_namespaced(client);
    // Stop on error including a pod already exists or still being deleted.
    pods.create(&PostParams::default(), &p).await?;

    // Wait until the pod is running, otherwise we get 500 error.
    let wp = WatchParams::default().fields("metadata.name=example").timeout(10);
    let mut stream = pods.watch(&wp, "0").await?.boxed();
    while let Some(status) = stream.try_next().await? {
        match status {
            WatchEvent::Added(o) => {
                info!("Added {}", o.name_any());
            }
            WatchEvent::Modified(o) => {
                let s = o.status.as_ref().expect("status exists on pod");
                if s.phase.clone().unwrap_or_default() == "Running" {
                    info!("Ready to attach to {}", o.name_any());
                    break;
                }
            }
            _ => {}
        }
    }

    let data = std::fs::read("./hello_world_aarch64").unwrap();
    let metadata = std::fs::metadata("./hello_world_aarch64")?;  // importantly, this file has executable permissions
    info!("Metadata: {:?}", metadata);

    let file_name = "rust_binary";

    // Write the data to pod
    {
        let mut header = tar::Header::new_gnu();
        header.set_path(file_name).unwrap();
        header.set_size(data.len() as u64);
        header.set_metadata(&metadata);
        header.set_cksum();

        let mut ar = tar::Builder::new(Vec::new());
        ar.append(&header, &mut data.as_slice()).unwrap();
        let data = ar.into_inner().unwrap();

        let ap = AttachParams::default().stdin(true).stderr(false);
        let mut tar = pods
            .exec("example", vec!["tar", "xf", "-", "-C", "/"], &ap)
            .await?;
        tar.stdin().unwrap().write_all(&data).await?;
    }

    // Check that the file was written
    {
        let ap = AttachParams::default().stderr(true);
        let mut cat = pods
            .exec("example", vec!["ls", "-l", &format!("/{file_name}")], &ap)
            .await?;
        let mut cat_out = tokio_util::io::ReaderStream::new(cat.stdout().unwrap());
        // let mut cat_out_err = tokio_util::io::ReaderStream::new(cat.stderr().unwrap());
        let next_stdout = cat_out.next().await.unwrap()?;

        info!("File permissions: {:?}", next_stdout);
    }
    {
      let ap = AttachParams::default().stderr(false);
      let mut cat = pods
          .exec("example", vec![&format!("./{file_name}")], &ap)
          .await?;
      let mut cat_out = tokio_util::io::ReaderStream::new(cat.stdout().unwrap());
      let next_stdout = cat_out.next().await.unwrap()?;

      info!("Logs from running hello world: {:?}", next_stdout);
  }

    // Clean up the pod
    // pods.delete("example", &DeleteParams::default())
    //     .await?
    //     .map_left(|pdel| {
    //         assert_eq!(pdel.name_any(), "example");
    //     });

    Ok(())
}
