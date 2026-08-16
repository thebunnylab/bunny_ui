//
//  CountriesDBRepository.swift
//  CountriesSwiftUI
//

#![allow(non_snake_case)]

use super::ModelContainer::{FetchDescriptor, MainDBRepository};
use crate::Repositories::Models::{ApiModel, DBModel};
use motor::loadable::LoadError;
use super::super::WebAPI::WebRepository::BoxFuture;

/// `protocol CountriesDBRepository`
pub trait CountriesDBRepository {
    fn countryDetails(
        &self,
        country: &DBModel::Country,
    ) -> BoxFuture<Result<Option<DBModel::CountryDetails>, LoadError>>;
    fn store(&self, countries: Vec<ApiModel::Country>) -> BoxFuture<Result<(), LoadError>>;
    fn storeDetails(
        &self,
        countryDetails: ApiModel::CountryDetails,
        country: &DBModel::Country,
    ) -> BoxFuture<Result<(), LoadError>>;
}

/// `extension MainDBRepository: CountriesDBRepository`
impl CountriesDBRepository for MainDBRepository {
    fn countryDetails(
        &self,
        country: &DBModel::Country,
    ) -> BoxFuture<Result<Option<DBModel::CountryDetails>, LoadError>> {
        let alpha3Code = country.alpha3Code.clone();
        let context = self.modelContext.clone();
        Box::pin(async move {
            // FetchDescriptor(predicate: #Predicate { $0.alpha3Code == alpha3Code })
            let descriptor = FetchDescriptor::<DBModel::CountryDetails>::new(move |details| {
                details.alpha3Code == alpha3Code
            });
            Ok(context.fetchDetails(&descriptor).into_iter().next())
        })
    }

    fn store(&self, countries: Vec<ApiModel::Country>) -> BoxFuture<Result<(), LoadError>> {
        let context = self.modelContext.clone();
        Box::pin(async move {
            context.transaction(|ctx| {
                for country in countries {
                    ctx.insertCountry(country.dbModel());
                }
            });
            Ok(())
        })
    }

    fn storeDetails(
        &self,
        countryDetails: ApiModel::CountryDetails,
        country: &DBModel::Country,
    ) -> BoxFuture<Result<(), LoadError>> {
        let alpha3Code = country.alpha3Code.clone();
        let context = self.modelContext.clone();
        Box::pin(async move {
            context.transaction(|ctx| {
                let currencies: Vec<DBModel::Currency> =
                    countryDetails.currencies.iter().map(|c| c.dbModel()).collect();
                let borders = countryDetails.borders.clone().unwrap_or_default();
                let neighborsFetch = FetchDescriptor::<DBModel::Country>::new(move |countryDBModel| {
                    borders.contains(&countryDBModel.alpha3Code)
                });
                let neighbors = ctx.fetchCountries(&neighborsFetch);
                for currency in &currencies {
                    ctx.insertCurrency(currency.clone());
                }
                let object = DBModel::CountryDetails::new(
                    alpha3Code,
                    countryDetails.capital,
                    currencies,
                    neighbors,
                );
                ctx.insertDetails(object);
            });
            Ok(())
        })
    }
}

// MARK: - internal extensions (ApiModel → DBModel)

/// `extension ApiModel.Country { func dbModel() -> DBModel.Country }`
pub trait CountryDBModel {
    fn dbModel(&self) -> DBModel::Country;
}

impl CountryDBModel for ApiModel::Country {
    fn dbModel(&self) -> DBModel::Country {
        DBModel::Country::new(
            self.name.clone(),
            self.translations.clone(),
            self.population,
            self.flag.clone(),
            self.alpha3Code.clone(),
        )
    }
}

/// `extension ApiModel.Currency { func dbModel() -> DBModel.Currency }`
pub trait CurrencyDBModel {
    fn dbModel(&self) -> DBModel::Currency;
}

impl CurrencyDBModel for ApiModel::Currency {
    fn dbModel(&self) -> DBModel::Currency {
        DBModel::Currency {
            code: self.code.clone(),
            symbol: self.symbol.clone(),
            name: self.name.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Repositories::Models::ApiModel;
    use motor::block_on;

    #[test]
    fn storesAndFetchesDetails() {
        let repository = MainDBRepository::new(crate::Repositories::Database::ModelContainer::ModelContainer::stub());
        let country =
            DBModel::Country::new("United States".into(), Default::default(), 1, None, "USA".into());
        let details = ApiModel::mockedCountryDetails().remove(0);
        block_on(repository.storeDetails(details, &country)).unwrap();

        let stored = block_on(repository.countryDetails(&country)).unwrap();
        assert!(stored.is_some());
        assert_eq!(stored.unwrap().capital, "Sin City");
    }

    #[test]
    fn storesCountries() {
        let repository = MainDBRepository::new(crate::Repositories::Database::ModelContainer::ModelContainer::stub());
        let apiCountries = ApiModel::mockedCountries();
        block_on(repository.store(apiCountries)).unwrap();

        let all = FetchDescriptor::<DBModel::Country>::new(|_| true);
        assert_eq!(repository.modelContext.fetchCountries(&all).len(), 3);
    }
}
