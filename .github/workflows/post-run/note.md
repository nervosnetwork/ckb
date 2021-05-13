```
CREATE TABLE IF NOT EXISTS benchmark (
    time                    TIMESTAMP       NOT NULL,
    send_delay              BIGINT          NOT NULL,
    tps                     BIGINT          NOT NULL,
    ckb_version             CHAR ( 40 )     NOT NULL,
    transaction_type        CHAR ( 40 )     NOT NULL,
    instances_count         INT             NOT NULL,
    total_transactions_size BIGINT          NOT NULL,
    instance_type           CHAR ( 40 )     NOT NULL,
    instance_bastion_type   CHAR ( 40 )     DEFAULT NULL
);
SELECT create_hypertable('benchmark', 'time');
```
