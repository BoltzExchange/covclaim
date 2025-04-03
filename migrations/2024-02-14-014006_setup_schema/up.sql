CREATE TABLE parameters (
    name VARCHAR PRIMARY KEY NOT NULL,
    value VARCHAR NOT NULL
);

CREATE TABLE pending_covenants (
    output_script BLOB PRIMARY KEY NOT NULL,
    status INT NOT NULL,
    internal_key BLOB NOT NULL,
    preimage BLOB NOT NULL,
    swap_tree VARCHAR NOT NULL,
    address BLOB NOT NULL,
    blinding_key BLOB,
    tx_id BLOB,
    tx_time DATETIME,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    swap_id VARCHAR NOT NULL
);

CREATE INDEX pending_covenants_status_idx ON pending_covenants (status);
