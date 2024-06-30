CREATE TABLE parameters (
    name VARCHAR PRIMARY KEY NOT NULL,
    value VARCHAR NOT NULL
);

CREATE TABLE pending_covenants (
    output_script BYTEA PRIMARY KEY NOT NULL,
    status INT NOT NULL,
    internal_key BYTEA NOT NULL,
    preimage BYTEA NOT NULL,
    swap_tree VARCHAR NOT NULL,
    address BYTEA NOT NULL,
    blinding_key BYTEA,
    tx_id BYTEA,
    tx_time TIMESTAMP,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX pending_covenants_status_idx ON pending_covenants (status);
