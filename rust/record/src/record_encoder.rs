// Copyright (C) 2019-2021 Aleo Systems Inc.
// This file is part of the Aleo library.

// The Aleo library is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// The Aleo library is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with the Aleo library. If not, see <https://www.gnu.org/licenses/>.

use crate::{CommitmentRandomness, Record, Payload, SerialNumberNonce};

use snarkvm_algorithms::traits::{CommitmentScheme, CRH};
use snarkvm_curves::{
    edwards_bls12::{EdwardsParameters, EdwardsProjective as EdwardsBls},
    AffineCurve,
    ProjectiveCurve,
};
use snarkvm_dpc::{
    base_dpc::instantiated::Components,
    decode_from_group,
    encode_to_group,
    DPCComponents,
    DPCError,
    Record as RecordInterface,
    RecordSerializerScheme,
};
use snarkvm_fields::PrimeField;
use snarkvm_utilities::{bits_to_bytes, bytes_to_bits, to_bytes, BigInteger, FromBytes, ToBytes};

use itertools::Itertools;

/// The decoded format of the Aleo record datatype.
/// Excludes the owner, commitment, and commitment_randomness from encoding.
pub struct DecodedRecord {
    pub payload: Payload,
    pub value: u64,

    pub birth_program_id: Vec<u8>,
    pub death_program_id: Vec<u8>,

    pub serial_number_nonce: SerialNumberNonce,
    pub commitment_randomness: CommitmentRandomness,
}

pub struct RecordEncoder;

impl RecordSerializerScheme for RecordEncoder {
    type DeserializedRecord = DecodedRecord;
    type Group = EdwardsBls;
    type InnerField = <Components as DPCComponents>::InnerField;
    type OuterField = <Components as DPCComponents>::OuterField;
    type Parameters = EdwardsParameters;
    type Record = Record;

    fn serialize(record: &Self::Record) -> Result<(Vec<Self::Group>, bool), DPCError> {
        // Assumption 1 - The scalar field bit size must be strictly less than the base field bit size
        // for the logic below to work correctly.
        // assert!(Self::SCALAR_FIELD_BITSIZE < Self::INNER_FIELD_BITSIZE);

        // Assumption 2 - this implementation assumes the outer field bit size is larger than
        // the data field bit size by at most one additional scalar field bit size.
        // assert!((Self::OUTER_FIELD_BITSIZE - Self::DATA_ELEMENT_BITSIZE) <= Self::DATA_ELEMENT_BITSIZE);

        // Assumption 3 - this implementation assumes the remainder of two outer field bit sizes
        // can fit within one data field element's bit size.
        // assert!((2 * (Self::OUTER_FIELD_BITSIZE - Self::DATA_ELEMENT_BITSIZE)) <= Self::DATA_ELEMENT_BITSIZE);

        // Assumption 4 - this implementation assumes the payload and value may be zero values.
        // As such, to ensure the values are non-zero for encoding and decoding, we explicitly
        // reserve the MSB of the data field element's valid bitsize and set the bit to 1.
        // assert_eq!(Self::PAYLOAD_ELEMENT_BITSIZE, Self::DATA_ELEMENT_BITSIZE - 1);

        // This element needs to be represented in the constraint field; its bits and the number of elements
        // are calculated early, so that the storage vectors can be pre-allocated.
        let payload = record.payload();
        let payload_bytes = to_bytes![payload]?;
        let payload_bits_count = payload_bytes.len() * 8;
        let payload_bits = bytes_to_bits(&payload_bytes);
        let num_payload_elements = payload_bits_count / Self::PAYLOAD_ELEMENT_BITSIZE;

        // Create the vector for storing data elements.

        let mut data_elements = Vec::with_capacity(5 + num_payload_elements + 2);
        let mut data_high_bits = Vec::with_capacity(5 + num_payload_elements);

        // These elements are already in the constraint field.

        let serial_number_nonce = record.serial_number_nonce();
        let serial_number_nonce_encoded =
            <Self::Group as ProjectiveCurve>::Affine::from_random_bytes(&to_bytes![serial_number_nonce]?.to_vec())
                .unwrap();

        data_elements.push(serial_number_nonce_encoded);
        data_high_bits.push(false);

        assert_eq!(data_elements.len(), 1);
        assert_eq!(data_high_bits.len(), 1);

        // These elements need to be represented in the constraint field.

        let commitment_randomness = record.commitment_randomness();
        let birth_program_id = record.birth_program_id();
        let death_program_id = record.death_program_id();
        let value = record.value();

        // Process commitment_randomness. (Assumption 1 applies)

        let (encoded_commitment_randomness, sign_high) =
            encode_to_group::<Self::Parameters, Self::Group>(&to_bytes![commitment_randomness]?[..])?;
        data_elements.push(encoded_commitment_randomness);
        data_high_bits.push(sign_high);

        assert_eq!(data_elements.len(), 2);
        assert_eq!(data_high_bits.len(), 2);

        // Process birth_program_id and death_program_id. (Assumption 2 and 3 applies)

        let birth_program_id_biginteger = Self::OuterField::read(birth_program_id)?.into_repr();
        let death_program_id_biginteger = Self::OuterField::read(death_program_id)?.into_repr();

        let mut birth_program_id_bits = Vec::with_capacity(Self::INNER_FIELD_BITSIZE);
        let mut death_program_id_bits = Vec::with_capacity(Self::INNER_FIELD_BITSIZE);
        let mut birth_program_id_remainder_bits =
            Vec::with_capacity(Self::OUTER_FIELD_BITSIZE - Self::DATA_ELEMENT_BITSIZE);
        let mut death_program_id_remainder_bits =
            Vec::with_capacity(Self::OUTER_FIELD_BITSIZE - Self::DATA_ELEMENT_BITSIZE);

        for i in 0..Self::DATA_ELEMENT_BITSIZE {
            birth_program_id_bits.push(birth_program_id_biginteger.get_bit(i));
            death_program_id_bits.push(death_program_id_biginteger.get_bit(i));
        }

        // (Assumption 2 applies)
        for i in Self::DATA_ELEMENT_BITSIZE..Self::OUTER_FIELD_BITSIZE {
            birth_program_id_remainder_bits.push(birth_program_id_biginteger.get_bit(i));
            death_program_id_remainder_bits.push(death_program_id_biginteger.get_bit(i));
        }
        birth_program_id_remainder_bits.append(&mut death_program_id_remainder_bits);

        // (Assumption 3 applies)

        let (encoded_birth_program_id, sign_high) =
            encode_to_group::<Self::Parameters, Self::Group>(&bits_to_bytes(&birth_program_id_bits)[..])?;
        drop(birth_program_id_bits);
        data_elements.push(encoded_birth_program_id);
        data_high_bits.push(sign_high);

        let (encoded_death_program_id, sign_high) =
            encode_to_group::<Self::Parameters, Self::Group>(&bits_to_bytes(&death_program_id_bits)[..])?;
        drop(death_program_id_bits);
        data_elements.push(encoded_death_program_id);
        data_high_bits.push(sign_high);

        let (encoded_birth_program_id_remainder, sign_high) =
            encode_to_group::<Self::Parameters, Self::Group>(&bits_to_bytes(&birth_program_id_remainder_bits)[..])?;
        drop(birth_program_id_remainder_bits);
        data_elements.push(encoded_birth_program_id_remainder);
        data_high_bits.push(sign_high);

        assert_eq!(data_elements.len(), 5);
        assert_eq!(data_high_bits.len(), 5);

        // Process payload.

        let mut payload_field_bits = Vec::with_capacity(Self::PAYLOAD_ELEMENT_BITSIZE + 1);

        for (i, bit) in payload_bits.enumerate() {
            payload_field_bits.push(bit);

            if (i > 0) && ((i + 1) % Self::PAYLOAD_ELEMENT_BITSIZE == 0) {
                // (Assumption 4)
                payload_field_bits.push(true);
                let (encoded_payload_field, sign_high) =
                    encode_to_group::<Self::Parameters, Self::Group>(&bits_to_bytes(&payload_field_bits)[..])?;

                data_elements.push(encoded_payload_field);
                data_high_bits.push(sign_high);

                payload_field_bits.clear();
            }
        }

        assert_eq!(data_elements.len(), 5 + num_payload_elements);
        assert_eq!(data_high_bits.len(), 5 + num_payload_elements);

        // Process payload remainder and value.

        // Determine if value can fit in current payload_field_bits.
        let value_does_not_fit =
            (payload_field_bits.len() + data_high_bits.len() + (std::mem::size_of_val(&value) * 8))
                > Self::PAYLOAD_ELEMENT_BITSIZE;

        if value_does_not_fit {
            // (Assumption 4)
            payload_field_bits.push(true);

            let (encoded_payload_field, fq_high) =
                encode_to_group::<Self::Parameters, Self::Group>(&bits_to_bytes(&payload_field_bits)[..])?;

            data_elements.push(encoded_payload_field);
            data_high_bits.push(fq_high);

            payload_field_bits.clear();
        }

        assert_eq!(
            data_elements.len(),
            5 + num_payload_elements + (value_does_not_fit as usize)
        );

        // Append the value bits and create the final base element.
        let value_bits = bytes_to_bits(&to_bytes![value]?).collect();

        // (Assumption 4)
        let final_element = [vec![true], data_high_bits, value_bits, payload_field_bits].concat();
        let (encoded_final_element, final_sign_high) =
            encode_to_group::<Self::Parameters, Self::Group>(&bits_to_bytes(&final_element)[..])?;

        data_elements.push(encoded_final_element);

        assert_eq!(
            data_elements.len(),
            5 + num_payload_elements + (value_does_not_fit as usize) + 1
        );

        // Compute the output group elements.

        let mut output = Vec::with_capacity(data_elements.len());
        for element in data_elements.iter() {
            output.push(element.into_projective());
        }

        Ok((output, final_sign_high))
    }

    fn deserialize(
        serialized_record: Vec<Self::Group>,
        final_sign_high: bool,
    ) -> Result<Self::DeserializedRecord, DPCError> {
        let remainder_size = Self::OUTER_FIELD_BITSIZE - Self::DATA_ELEMENT_BITSIZE;

        // Extract the fq_bits
        let final_element = &serialized_record[serialized_record.len() - 1];
        let final_element_bytes =
            decode_from_group::<Self::Parameters, Self::Group>(final_element.into_affine(), final_sign_high)?;
        let final_element_bits = bytes_to_bits(&final_element_bytes).collect::<Vec<_>>();

        let fq_high_bits = &final_element_bits[1..serialized_record.len()];

        // Deserialize serial number nonce

        let (serial_number_nonce, _) = &(serialized_record[0], fq_high_bits[0]);
        let serial_number_nonce_bytes = to_bytes![serial_number_nonce.into_affine().to_x_coordinate()]?;
        let serial_number_nonce =
            <<Components as DPCComponents>::SerialNumberNonceCRH as CRH>::Output::read(&serial_number_nonce_bytes[..])?;

        // Deserialize commitment randomness

        let (commitment_randomness, commitment_randomness_fq_high) = &(serialized_record[1], fq_high_bits[1]);
        let commitment_randomness_bytes = decode_from_group::<Self::Parameters, Self::Group>(
            commitment_randomness.into_affine(),
            *commitment_randomness_fq_high,
        )?;
        let commitment_randomness_bits = &bytes_to_bits(&commitment_randomness_bytes)
            .take(Self::DATA_ELEMENT_BITSIZE)
            .collect::<Vec<_>>();
        let commitment_randomness =
            <<Components as DPCComponents>::RecordCommitment as CommitmentScheme>::Randomness::read(
                &bits_to_bytes(commitment_randomness_bits)[..],
            )?;

        // Deserialize birth and death programs

        let (birth_program_id, birth_program_id_sign_high) = &(serialized_record[2], fq_high_bits[2]);
        let birth_program_id_bytes = decode_from_group::<Self::Parameters, Self::Group>(
            birth_program_id.into_affine(),
            *birth_program_id_sign_high,
        )?;

        let (death_program_id, death_program_id_sign_high) = &(serialized_record[3], fq_high_bits[3]);
        let death_program_id_bytes = decode_from_group::<Self::Parameters, Self::Group>(
            death_program_id.into_affine(),
            *death_program_id_sign_high,
        )?;

        let (program_id_remainder, program_id_sign_high) = &(serialized_record[4], fq_high_bits[4]);
        let program_id_remainder_bytes = decode_from_group::<Self::Parameters, Self::Group>(
            program_id_remainder.into_affine(),
            *program_id_sign_high,
        )?;

        let mut birth_program_id_bits = bytes_to_bits(&birth_program_id_bytes)
            .take(Self::DATA_ELEMENT_BITSIZE)
            .collect::<Vec<_>>();
        let mut death_program_id_bits = bytes_to_bits(&death_program_id_bytes)
            .take(Self::DATA_ELEMENT_BITSIZE)
            .collect::<Vec<_>>();

        let mut program_id_remainder_bits = bytes_to_bits(&program_id_remainder_bytes);
        birth_program_id_bits.extend(program_id_remainder_bits.by_ref().take(remainder_size));
        death_program_id_bits.extend(program_id_remainder_bits.take(remainder_size));

        let birth_program_id = bits_to_bytes(&birth_program_id_bits);
        let death_program_id = bits_to_bytes(&death_program_id_bits);

        // Deserialize the value

        let value_start = serialized_record.len();
        let value_end = value_start + (std::mem::size_of_val(&<Self::Record as RecordInterface>::Value::default()) * 8);
        let value: <Self::Record as RecordInterface>::Value =
            FromBytes::read(&bits_to_bytes(&final_element_bits[value_start..value_end])[..])?;

        // Deserialize payload

        let mut payload_bits = vec![];
        for (element, fq_high) in serialized_record[5..serialized_record.len() - 1]
            .iter()
            .zip_eq(&fq_high_bits[5..])
        {
            let element_bytes = decode_from_group::<Self::Parameters, Self::Group>(element.into_affine(), *fq_high)?;
            payload_bits.extend(bytes_to_bits(&element_bytes).take(Self::PAYLOAD_ELEMENT_BITSIZE));
        }
        payload_bits.extend_from_slice(&final_element_bits[value_end..]);

        let payload = Payload::read(&bits_to_bytes(&payload_bits)[..])?;

        Ok(DecodedRecord {
            payload,
            value,
            birth_program_id,
            death_program_id,
            serial_number_nonce,
            commitment_randomness,
        })
    }
}

impl From<Record> for DecodedRecord {
    fn from(record: Record) -> Self {
        Self {
            payload: record.payload,
            value: record.value,
            birth_program_id: record.birth_program_id,
            death_program_id: record.death_program_id,
            serial_number_nonce: record.serial_number_nonce,
            commitment_randomness: record.commitment_randomness,
        }
    }
}
