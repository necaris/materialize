// Copyright Materialize, Inc. and contributors. All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

use mz_interchange::protobuf::{DecodedDescriptors, Decoder};
use mz_repr::Row;

use crate::types::errors::DecodeErrorKind;
use crate::types::sources::encoding::ProtobufEncoding;

#[derive(Debug)]
pub struct ProtobufDecoderState {
    decoder: Decoder,
    events_success: i64,
    events_error: i64,
}

impl ProtobufDecoderState {
    pub fn new(
        ProtobufEncoding {
            descriptors,
            message_name,
            confluent_wire_format,
        }: ProtobufEncoding,
    ) -> Result<Self, anyhow::Error> {
        let descriptors = DecodedDescriptors::from_bytes(&descriptors, message_name)
            .expect("descriptors provided to protobuf source are pre-validated");
        Ok(ProtobufDecoderState {
            decoder: Decoder::new(descriptors, confluent_wire_format)?,
            events_success: 0,
            events_error: 0,
        })
    }
    pub fn get_value(&mut self, bytes: &[u8]) -> Option<Result<Row, DecodeErrorKind>> {
        match self.decoder.decode(bytes) {
            Ok(row) => {
                if let Some(row) = row {
                    self.events_success += 1;
                    Some(Ok(row))
                } else {
                    self.events_error += 1;
                    Some(Err(DecodeErrorKind::Text(format!(
                        "protobuf deserialization returned None"
                    ))))
                }
            }
            Err(err) => {
                self.events_error += 1;
                Some(Err(DecodeErrorKind::Text(format!(
                    "protobuf deserialization error: {:#}",
                    err
                ))))
            }
        }
    }
}
