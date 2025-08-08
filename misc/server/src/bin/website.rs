use anyhow::{anyhow, Result};
use futures::stream::{StreamExt, TryStreamExt};
use ipfs_api::{IpfsApi, IpfsClient};
use std::path::Path;
use tokio::fs;
//use tokio::io;
use std::env;
use std::io::Cursor;
use std::io::{self, Write};

//async fn upload_directory_to_ipfs(
//    ipfs: &IpfsClient,
//    dir_path: &Path,
//) -> Result<String> {
//    let mut files = fs::read_dir(dir_path).await?;
//    let mut file_paths = Vec::new();
//
//    while let Some(entry) = files.next_entry().await? {
//        let path = entry.path();
//        file_paths.push(path);
//    }
//
//    let results = ipfs
//        .add_files_recursive(file_paths.iter())
//        .map_ok(|added_file| added_file.hash)
//        .try_collect::<Vec<String>>()
//        .await?;
//
//    // Assuming the directory itself is the last item in the list.
//    if let Some(root_cid) = results.last() {
//        Ok(root_cid.clone())
//    } else {
//        Err(anyhow!("Failed to get root CID from IPFS upload."))
//    }
//}

#[tokio::main]
async fn main() -> Result<()> {
    let client = IpfsClient::default();

    let res = client.id(None).await;
    println!("{:?}", res);
    let res = client
        .id(Some("12D3KooWSyHa263MdiSuFACFRkz54SDxHqzjdyGKQSu9gTZxGXs8"))
        .await;

    println!("{:?}", res);

    let data = Cursor::new("Hello World!");
    match client.add(data).await {
        Ok(res) => {
            println!("{}", res.hash);
            //let output_path = Path::new(&res.hash); // Use CID as filename/directory

            let srcdir = PathBuf::from("./src");
            let path = format!("{}", Path::new(&srcdir).join("file.json").display());
            println!("{:?}", fs::canonicalize(&path));
            //https://bafybeib66hlejohsjfm2wuxnsld5m47nkzhy6ffx3vzuwq323skkxt55yu.ipfs.dweb.link
            match client
                .get(&path)
                .map_ok(|chunk| chunk.to_vec())
                .try_concat()
                .await
            {
                Ok(res) => {
                    let out = io::stdout();
                    let mut out = out.lock();

                    out.write_all(&res).unwrap();
                }
                Err(e) => eprintln!("error getting file: {}", e),
            }
        }
        Err(e) => eprintln!("error adding file: {}", e),
    }

    let srcdir = PathBuf::from("./src");
    println!("{:?}", fs::canonicalize(&srcdir));

    //let solardir = PathBuf::from("../misc");
    //println!("{:?}", fs::canonicalize(&solardir));

    let path = format!("{}", Path::new(&srcdir).join("file.json").display());
    println!("{:?}", fs::canonicalize(&path));

    use std::fs;
    use std::path::PathBuf;

    let srcdir = PathBuf::from("./src");
    println!("{:?}", fs::canonicalize(&srcdir));

    //let solardir = PathBuf::from("../misc");
    //println!("{:?}", fs::canonicalize(&solardir));

    let path = Path::new(&srcdir).join("file.json");
    println!("{}", path.display());

    //match env::current_dir() {
    //    Ok(path) => {
    //        println!("Current directory: {}", path.display());
    //    }
    //    Err(e) => {
    //        eprintln!("Failed to get current directory: {}", e);
    //    }
    //}

    //match client
    //    .get(path.to_str().unwrap_or(""))
    //    .map_ok(|chunk| chunk.to_vec())
    //    .try_concat()
    //    .await
    //{
    //    Ok(res) => {
    //        let out = io::stdout();
    //        let mut out = out.lock();

    //        //out.write_all(&res).unwrap();
    //        println!("{:?}", &res);
    //    }
    //    Err(e) => eprintln!("error getting file: {}", e),
    //}

    //    let ipfs = IpfsClient::default(); // Connect to local IPFS node
    //
    //    let website_dir = Path::new("./src/bin"); // Replace with your website directory
    //
    //    if !website_dir.exists() {
    //        fs::create_dir_all(website_dir).await?;
    //        fs::write(website_dir.join("index.html"), "<h1>Hello from IPFS!</h1>").await?;
    //        fs::write(website_dir.join("style.css"), "body { background-color: lightblue; }").await?;
    //    }
    //
    //    let cid = upload_directory_to_ipfs(&ipfs, website_dir).await?;
    //
    //    println!("Website uploaded to IPFS with CID: {}", cid);
    //    println!("Access it via: https://ipfs.io/ipfs/{}", cid);
    //    println!("Or via a local gateway: http://127.0.0.1:8080/ipfs/{}", cid);

    Ok(())
}
