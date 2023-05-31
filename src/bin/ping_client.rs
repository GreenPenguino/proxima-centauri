use clap::Parser;
use std::net::SocketAddrV4;
use std::str::FromStr;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::sleep,
};

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let in_transit = Arc::new(AtomicBool::new(false));
    let in_transit2 = in_transit.clone();
    let out_timestamp = Arc::new(Mutex::new(Instant::now()));
    let out_timestamp2 = out_timestamp.clone();
    let count = AtomicU32::new(args.count);

    let addr = SocketAddrV4::from_str(&args.address).unwrap();

    let stream = TcpStream::connect(addr).await.unwrap();
    if !args.csv {
        println!("Ping {addr}");
    }

    let (mut si, mut so) = stream.into_split();

    let ping_in = async move {
        let mut read_buf = [0; 1024];
        while count.load(Ordering::Relaxed) > 0 {
            let bytes = si.read(&mut read_buf).await.unwrap();
            if bytes > 0 {
                let in_timestamp = Instant::now();
                // println!("Received {bytes} bytes");

                let duration = in_timestamp.duration_since(*out_timestamp.lock().unwrap());
                let i = u32::from_be_bytes(read_buf[0..bytes].try_into().unwrap());
                let rtt = duration.as_micros();
                if !args.csv {
                    println!("Ping {i} arrived with RTT of {rtt}us");
                } else {
                    println!("{rtt},");
                }
                // let old = count.fetch_sub(1, Ordering::Relaxed);
                // println!("Old count value: {old}");
                count.fetch_sub(1, Ordering::Relaxed);
                in_transit2.store(false, Ordering::Relaxed);
            }
        }
        if !args.csv {
            println!("Done receiving");
        }
    };

    let ping_out = async move {
        for i in 1..=args.count {
            if let Ok(false) =
                in_transit.compare_exchange_weak(false, true, Ordering::Relaxed, Ordering::Relaxed)
            {
                *out_timestamp2.lock().unwrap() = Instant::now();
                so.write_u32(i).await.unwrap();
            }

            if !args.csv {
                println!("Sending ping {i}");
            }
            sleep(Duration::from_millis(1)).await;
        }
        if !args.csv {
            println!("Done sending pings");
        }
    };

    tokio::join!(ping_out, ping_in);
}

#[derive(Parser, Debug)]
struct Args {
    /// Socket address to ping
    #[arg(long)]
    address: String,

    /// Amount of pings to send
    #[arg(short, long)]
    count: u32,

    /// CSV mode
    #[arg(long)]
    csv: bool,
}
