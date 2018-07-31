# nervos_explorer

> nervos block explorer

## Build Setup

``` bash
# install dependencies
npm install

# serve with hot reload at localhost:8080
npm run dev

# build for production with minification
npm run build
```

## How to send transaction

start node with development config file:
```
cp src/config/development.toml /tmp/node1/config.toml

cargo run -- --data-dir=/tmp/node1
```

visit http://localhost:8080/ , click `SEND TRANSACTION` tab, edit previous_output hash and outputs lock, click `SEND`
