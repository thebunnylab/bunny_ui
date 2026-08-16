//
//  AppSchema.swift
//  CountriesSwiftUI
//

#![allow(non_snake_case)]

/// `Schema` fake — the model list plus a version, used only for fidelity.
#[derive(Clone, Debug, PartialEq)]
pub struct Schema {
    pub version: SchemaVersion,
}

/// `Schema.Version(1, 0, 0)`
#[derive(Clone, Debug, PartialEq)]
pub struct SchemaVersion(pub i64, pub i64, pub i64);

pub fn Version(_major: i64, _minor: i64, _patch: i64) -> SchemaVersion {
    SchemaVersion(_major, _minor, _patch)
}

impl Schema {
    /// `static var appSchema: Schema`
    pub fn appSchema() -> Schema {
        let actualVersion = Version(1, 0, 0);
        // Schema([DBModel.Country.self, DBModel.CountryDetails.self,
        //          DBModel.Currency.self], version:)
        Schema { version: actualVersion }
    }
}
