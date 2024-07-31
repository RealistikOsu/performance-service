# Performance Service

Multi-purpose microservice for all things performance.

It features:
- A performance calculator (compliant with [USSR](https://github.com/RealistikOsu/USSR)).
- A mass score recalculator.

## Usage

### Requirements
- The Rust toolchain installed (mainly `cargo`) for compilation.

### Compilation
To compile it, use the following command if you want simply use it for testing. This will reduce compile times.
```sh
cargo build
```

This will create a binary in `performance-service/target/debug`.

Or if you want a production deployment, use this one. It will take longer (~1 minute excluding libraries) but produce a significantly faster binary.
```sh
cargo build --release
```

This will create a binary in `performance-service/target/release`.

### Deployment

#### Components
The performance service features multiple componenets to cover most usecases. These can be selected through the `app-component` flag as so:
`./performance-service --app-component=thing`.

| Component | Usage |
| ---| --- |
| `api` | Runs a web API for performance calculations. Use when running a score server such as [USSR](https://github.com/RealistikOsu/USSR) |
| `deploy` | A console interface for recalculating the whole server with the current PP system. Handles stats and score overwrite. |
| `mass_recalc` | **Internal** undocumented rework testing tool. |
| `recalc` | Recalculates a single user. |

#### Configuration
There are other flags available for advanced users and configuration. **These may also be set through creating a copy of the `.env.example` file and naming it `.env`.**

| Flag | Purpose |
| ---| --- |
| `api_port` | The port at which the `api` component should run. Has no impact elsewhere. |
| `mysql_user` | A MySQL user with read-write access to scores and users. |
| `mysql_password` | The password for the MySQL user. |
| `mysql_host` | The host of the MySQL server. |
| `mysql_port` | The port of the MySQL server. |
| `mysql_database` | The RealistikOsu database. |
| `redis_user` | The Redis user with access to global readerboards (leave empty if no authentification is required). |
| `redis_password` | The password for the Redis user. |
| `redis_host` | The host of the Redis server used for global leaderboards. |
| `redis_port` | The port of the Redis server. |
| `redis_db` | The index of the Redis database. |
| `beatmaps_path` | The location at which `.osu` files for beatmaps should be stored. For optimum performance, use the same directory that your score server uses. |

