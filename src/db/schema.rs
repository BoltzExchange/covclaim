// @generated automatically by Diesel CLI.

diesel::table! {
    parameters (name) {
        name -> Varchar,
        value -> Varchar,
    }
}

diesel::table! {
    pending_covenants (output_script) {
        output_script -> Bytea,
        status -> Int4,
        internal_key -> Bytea,
        preimage -> Bytea,
        swap_tree -> Varchar,
        address -> Bytea,
        blinding_key -> Nullable<Bytea>,
        tx_id -> Nullable<Bytea>,
        tx_time -> Nullable<Timestamp>,
        created_at -> Timestamp,
        swap_id -> Varchar,
    }
}

diesel::allow_tables_to_appear_in_same_query!(
    parameters,
    pending_covenants,
);
