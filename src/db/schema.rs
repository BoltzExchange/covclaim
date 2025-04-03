// @generated automatically by Diesel CLI.

diesel::table! {
    parameters (name) {
        name -> Text,
        value -> Text,
    }
}

diesel::table! {
    pending_covenants (output_script) {
        output_script -> Binary,
        status -> Integer,
        internal_key -> Binary,
        preimage -> Binary,
        swap_tree -> Text,
        address -> Binary,
        blinding_key -> Nullable<Binary>,
        tx_id -> Nullable<Binary>,
        tx_time -> Nullable<Timestamp>,
        created_at -> Timestamp,
        swap_id -> Text,
    }
}

diesel::allow_tables_to_appear_in_same_query!(parameters, pending_covenants,);
