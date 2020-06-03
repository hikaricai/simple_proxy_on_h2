### Introduction
>A simple proxy demo based on http2 to learn rust and tokio0.2
### QuickStart
Prepare Rust development environment

Clone this repo and cd into it

Run the following command
```
Server: cargo run server
Client: cargo run
ssh username@localhost -p2022
```
The above commands will establish port forwarding ssh channel as:

> ssh_client -> **Client(2022/tcp) -> Server** -> TARGET_IP:(22/tcp)  

which tunnels the original connection:

> ssh_client -> TARGET_IP:(22/tcp)  
