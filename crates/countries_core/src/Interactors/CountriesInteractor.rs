//
//  CountriesInteractor.swift
//  CountriesSwiftUI
//

#![allow(non_snake_case)]

use crate::Repositories::Database::CountriesDBRepository::CountriesDBRepository;
use crate::Repositories::Models::DBModel;
use crate::Repositories::WebAPI::CountriesWebRepository::CountriesWebRepository;
use crate::Repositories::WebAPI::WebRepository::BoxFuture;
use motor::loadable::LoadError;

/// `struct ValueIsMissingError()` — usado como falha quando o store não devolve
/// o que acabou de ser gravado.
pub fn ValueIsMissingError() -> LoadError {
    LoadError::new("Value is missing")
}

/// `protocol CountriesInteractor`
pub trait CountriesInteractor {
    fn refreshCountriesList(&self) -> BoxFuture<Result<(), LoadError>>;
    fn loadCountryDetails(
        &self,
        country: DBModel::Country,
        forceReload: bool,
    ) -> BoxFuture<Result<DBModel::CountryDetails, LoadError>>;
}

pub struct RealCountriesInteractor {
    pub webRepository: Rc<dyn CountriesWebRepository>,
    pub dbRepository: Rc<dyn CountriesDBRepository>,
}

impl RealCountriesInteractor {
    pub fn new(
        webRepository: Rc<dyn CountriesWebRepository>,
        dbRepository: Rc<dyn CountriesDBRepository>,
    ) -> Self {
        RealCountriesInteractor { webRepository, dbRepository }
    }
}

impl CountriesInteractor for RealCountriesInteractor {
    fn refreshCountriesList(&self) -> BoxFuture<Result<(), LoadError>> {
        let web = self.webRepository.clone();
        let db = self.dbRepository.clone();
        Box::pin(async move {
            let apiCountries = web.countries().await?;
            db.store(apiCountries).await?;
            Ok(())
        })
    }

    fn loadCountryDetails(
        &self,
        country: DBModel::Country,
        forceReload: bool,
    ) -> BoxFuture<Result<DBModel::CountryDetails, LoadError>> {
        let web = self.webRepository.clone();
        let db = self.dbRepository.clone();
        Box::pin(async move {
            if !forceReload {
                if let Ok(Some(stored)) = db.countryDetails(&country).await {
                    return Ok(stored);
                }
            }
            let details = web.details(&country).await?;
            db.storeDetails(details, &country).await?;
            match db.countryDetails(&country).await {
                Ok(Some(stored)) => Ok(stored),
                _ => Err(ValueIsMissingError()),
            }
        })
    }
}

pub struct StubCountriesInteractor;

impl CountriesInteractor for StubCountriesInteractor {
    fn refreshCountriesList(&self) -> BoxFuture<Result<(), LoadError>> {
        Box::pin(async { Ok(()) })
    }

    fn loadCountryDetails(
        &self,
        _country: DBModel::Country,
        _forceReload: bool,
    ) -> BoxFuture<Result<DBModel::CountryDetails, LoadError>> {
        Box::pin(async { Err(ValueIsMissingError()) })
    }
}

use std::rc::Rc;

// MARK: - UnitTests

/// Port de `UnitTests/Mocks/{Mock,MockedWebRepositories,MockedDBRepositories}.swift`
/// + `UnitTests/Mocks/Interactors/CountriesInteractorTests.swift`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Repositories::Database::CountriesDBRepository::CountriesDBRepository;
    use crate::Repositories::Models::ApiModel;
    use crate::Repositories::WebAPI::WebRepository::{RealUrlSession, WebRepository};
    use motor::block_on;
    use motor::loadable::LoadError;
    use std::cell::RefCell;
    use std::fmt::Debug;

    // MARK: - TestHelpers.swift

    /// `enum MockError: Swift.Error { case valueNotSet; case codeDataModel }`
    enum MockError {
        valueNotSet,
        #[allow(dead_code)]
        codeDataModel,
    }

    impl MockError {
        fn error(&self) -> LoadError {
            match self {
                MockError::valueNotSet => LoadError::new("Value not set"),
                MockError::codeDataModel => LoadError::new("Code data model"),
            }
        }
    }

    /// `extension NSError { static var test: NSError }`
    fn NSError_test() -> LoadError {
        LoadError::new("Test error")
    }

    // MARK: - Mock.swift

    /// `final class MockActions<Action> where Action: Equatable`
    pub struct MockActions<Action: PartialEq + Debug> {
        expected: RefCell<Vec<Action>>,
        factual: RefCell<Vec<Action>>,
    }

    impl<Action: PartialEq + Debug> MockActions<Action> {
        /// `init(expected: [Action])`
        pub fn new(expected: Vec<Action>) -> Self {
            MockActions {
                expected: RefCell::new(expected),
                factual: RefCell::new(Vec::new()),
            }
        }

        /// `mock.actions = .init(expected: …)` — replaces the expectations
        pub fn setExpected(&self, expected: Vec<Action>) {
            *self.expected.borrow_mut() = expected;
        }

        /// `func register(_ action: Action)`
        pub fn register(&self, action: Action) {
            self.factual.borrow_mut().push(action);
        }

        /// `func verify()` — `#expect(factual == expected)`
        pub fn verify(&self) {
            assert_eq!(*self.factual.borrow(), *self.expected.borrow());
        }
    }

    // MARK: - MockedWebRepositories.swift

    /// `MockedCountriesWebRepository.Action`
    #[derive(Clone, Debug, PartialEq)]
    pub enum MockedCountriesWebRepositoryAction {
        countries,
        details { country: DBModel::Country },
    }

    /// `final class MockedCountriesWebRepository: TestWebRepository, Mock, CountriesWebRepository`
    pub struct MockedCountriesWebRepository {
        pub actions: MockActions<MockedCountriesWebRepositoryAction>,
        pub countriesResponses: RefCell<Vec<Result<Vec<ApiModel::Country>, LoadError>>>,
        pub detailsResponses: RefCell<Vec<Result<ApiModel::CountryDetails, LoadError>>>,
    }

    impl MockedCountriesWebRepository {
        /// `init()`
        pub fn init() -> Self {
            MockedCountriesWebRepository {
                actions: MockActions::new(Vec::new()),
                countriesResponses: RefCell::new(Vec::new()),
                detailsResponses: RefCell::new(Vec::new()),
            }
        }
    }

    /// `class TestWebRepository: WebRepository { session = .mockedResponsesOnly; baseURL }`
    impl WebRepository for MockedCountriesWebRepository {
        fn session(&self) -> Rc<dyn super::super::super::Repositories::WebAPI::WebRepository::UrlSession> {
            Rc::new(RealUrlSession)
        }
        fn baseURL(&self) -> String {
            "https://test.com".into()
        }
    }

    impl CountriesWebRepository for MockedCountriesWebRepository {
        fn countries(&self) -> BoxFuture<Result<Vec<ApiModel::Country>, LoadError>> {
            self.actions.register(MockedCountriesWebRepositoryAction::countries);
            let result = match self.countriesResponses.borrow_mut().pop_front() {
                Some(result) => result,
                None => Err(MockError::valueNotSet.error()),
            };
            Box::pin(async move { result })
        }

        fn details(
            &self,
            country: &DBModel::Country,
        ) -> BoxFuture<Result<ApiModel::CountryDetails, LoadError>> {
            self.actions
                .register(MockedCountriesWebRepositoryAction::details { country: country.clone() });
            let result = match self.detailsResponses.borrow_mut().pop_front() {
                Some(result) => result,
                None => Err(MockError::valueNotSet.error()),
            };
            Box::pin(async move { result })
        }
    }

    // MARK: - MockedDBRepositories.swift

    /// `MockedCountriesDBRepository.Action`
    #[derive(Clone, Debug, PartialEq)]
    pub enum MockedCountriesDBRepositoryAction {
        fetchCountryDetails(DBModel::Country),
        storeCountries(Vec<ApiModel::Country>),
        storeDetails { countryDetails: ApiModel::CountryDetails, country: DBModel::Country },
    }

    /// `final class MockedCountriesDBRepository: Mock, CountriesDBRepository`
    pub struct MockedCountriesDBRepository {
        pub actions: MockActions<MockedCountriesDBRepositoryAction>,
        pub storeCountriesResults: RefCell<Vec<Result<(), LoadError>>>,
        pub storeCountryDetailsResults: RefCell<Vec<Result<(), LoadError>>>,
        pub countryDetailsResults: RefCell<Vec<Result<Option<DBModel::CountryDetails>, LoadError>>>,
    }

    impl MockedCountriesDBRepository {
        /// `init()`
        pub fn init() -> Self {
            MockedCountriesDBRepository {
                actions: MockActions::new(Vec::new()),
                storeCountriesResults: RefCell::new(Vec::new()),
                storeCountryDetailsResults: RefCell::new(Vec::new()),
                countryDetailsResults: RefCell::new(Vec::new()),
            }
        }
    }

    impl CountriesDBRepository for MockedCountriesDBRepository {
        /// `func countryDetails(for country:) async throws -> DBModel.CountryDetails?`
        fn countryDetails(
            &self,
            country: &DBModel::Country,
        ) -> BoxFuture<Result<Option<DBModel::CountryDetails>, LoadError>> {
            self.actions
                .register(MockedCountriesDBRepositoryAction::fetchCountryDetails(country.clone()));
            let result = match self.countryDetailsResults.borrow_mut().pop_front() {
                Some(result) => result,
                None => Err(MockError::valueNotSet.error()),
            };
            Box::pin(async move { result })
        }

        /// `func store(countries:) async throws`
        fn store(&self, countries: Vec<ApiModel::Country>) -> BoxFuture<Result<(), LoadError>> {
            self.actions
                .register(MockedCountriesDBRepositoryAction::storeCountries(countries));
            let result = match self.storeCountriesResults.borrow_mut().pop_front() {
                Some(result) => result,
                None => Err(MockError::valueNotSet.error()),
            };
            Box::pin(async move { result })
        }

        /// `func store(countryDetails:for:) async throws`
        fn storeDetails(
            &self,
            countryDetails: ApiModel::CountryDetails,
            country: &DBModel::Country,
        ) -> BoxFuture<Result<(), LoadError>> {
            self.actions.register(MockedCountriesDBRepositoryAction::storeDetails {
                countryDetails,
                country: country.clone(),
            });
            let result = match self.storeCountryDetailsResults.borrow_mut().pop_front() {
                Some(result) => result,
                None => Err(MockError::valueNotSet.error()),
            };
            Box::pin(async move { result })
        }
    }

    /// `removeFirst()` on a `Vec` — Swift's deque semantics (FIFO).
    trait PopFront {
        type Item;
        fn pop_front(&mut self) -> Option<Self::Item>;
    }

    impl<T> PopFront for Vec<T> {
        type Item = T;
        fn pop_front(&mut self) -> Option<T> {
            if self.is_empty() {
                None
            } else {
                Some(self.remove(0))
            }
        }
    }

    // MARK: - CountriesInteractorTests.swift

    /// `@Suite class CountriesInteractorTests { let mockedWebRepo; let mockedDBRepo; let sut }`
    struct CountriesInteractorTests {
        mockedWebRepo: Rc<MockedCountriesWebRepository>,
        mockedDBRepo: Rc<MockedCountriesDBRepository>,
        sut: RealCountriesInteractor,
    }

    impl CountriesInteractorTests {
        /// `init()`
        fn init() -> Self {
            let mockedWebRepo = Rc::new(MockedCountriesWebRepository::init());
            let mockedDBRepo = Rc::new(MockedCountriesDBRepository::init());
            let sut = RealCountriesInteractor::new(
                mockedWebRepo.clone(),
                mockedDBRepo.clone(),
            );
            CountriesInteractorTests { mockedWebRepo, mockedDBRepo, sut }
        }
    }

    use crate::Repositories::Database::CountriesDBRepository::{CountryDBModel, CurrencyDBModel};
    use crate::Repositories::Models::DBModel;

    /// `// MARK: - refreshCountriesList()`
    mod RefreshCountriesListTests {
        use super::*;

        #[test]
        fn happyPath() {
            let fixture = CountriesInteractorTests::init();
            let countries = ApiModel::mockedCountries();
            fixture.mockedWebRepo.actions.setExpected(vec![
                MockedCountriesWebRepositoryAction::countries,
            ]);
            *fixture.mockedWebRepo.countriesResponses.borrow_mut() =
                vec![Ok(countries.clone())];
            fixture.mockedDBRepo.actions.setExpected(vec![
                MockedCountriesDBRepositoryAction::storeCountries(countries),
            ]);
            *fixture.mockedDBRepo.storeCountriesResults.borrow_mut() = vec![Ok(())];
            block_on(fixture.sut.refreshCountriesList()).unwrap();
            fixture.mockedWebRepo.actions.verify();
            fixture.mockedDBRepo.actions.verify();
        }

        #[test]
        fn dbFailure() {
            let fixture = CountriesInteractorTests::init();
            let countries = ApiModel::mockedCountries();
            fixture.mockedWebRepo.actions.setExpected(vec![
                MockedCountriesWebRepositoryAction::countries,
            ]);
            *fixture.mockedWebRepo.countriesResponses.borrow_mut() =
                vec![Ok(countries.clone())];
            fixture.mockedDBRepo.actions.setExpected(vec![
                MockedCountriesDBRepositoryAction::storeCountries(countries),
            ]);
            let error = NSError_test();
            *fixture.mockedDBRepo.storeCountriesResults.borrow_mut() =
                vec![Err(error.clone())];
            // `await #expect(throws: error) { try await sut.refreshCountriesList() }`
            assert_eq!(block_on(fixture.sut.refreshCountriesList()).unwrap_err(), error);
            fixture.mockedWebRepo.actions.verify();
            fixture.mockedDBRepo.actions.verify();
        }

        #[test]
        fn webFailure() {
            let fixture = CountriesInteractorTests::init();
            fixture.mockedWebRepo.actions.setExpected(vec![
                MockedCountriesWebRepositoryAction::countries,
            ]);
            let error = NSError_test();
            *fixture.mockedWebRepo.countriesResponses.borrow_mut() =
                vec![Err(error.clone())];
            fixture.mockedDBRepo.actions.setExpected(vec![]);
            assert_eq!(block_on(fixture.sut.refreshCountriesList()).unwrap_err(), error);
            fixture.mockedWebRepo.actions.verify();
            fixture.mockedDBRepo.actions.verify();
        }
    }

    /// `// MARK: - loadCountryDetails(country: DBModel.Country, forceReload: Bool)`
    mod LoadCountryDetailsTests {
        use super::*;

        /// the `dbDetails` every cached-path test expects back
        fn dbDetails(country: &DBModel::Country, details: &ApiModel::CountryDetails) -> DBModel::CountryDetails {
            DBModel::CountryDetails::new(
                country.alpha3Code.clone(),
                details.capital.clone(),
                details.currencies.iter().map(|c| c.dbModel()).collect(),
                vec![],
            )
        }

        #[test]
        fn happyPathCachedData() {
            let fixture = CountriesInteractorTests::init();
            let country = ApiModel::mockedCountries().remove(0).dbModel();
            let details = ApiModel::mockedCountryDetails().remove(0);
            fixture.mockedWebRepo.actions.setExpected(vec![]);
            fixture.mockedDBRepo.actions.setExpected(vec![
                MockedCountriesDBRepositoryAction::fetchCountryDetails(country.clone()),
            ]);
            let expected = dbDetails(&country, &details);
            *fixture.mockedDBRepo.countryDetailsResults.borrow_mut() =
                vec![Ok(Some(expected.clone()))];
            let result = block_on(fixture.sut.loadCountryDetails(country.clone(), false)).unwrap();
            assert_eq!(result, expected);
            fixture.mockedWebRepo.actions.verify();
            fixture.mockedDBRepo.actions.verify();
        }

        #[test]
        fn happyPathCachedDataForceReload() {
            let fixture = CountriesInteractorTests::init();
            let country = ApiModel::mockedCountries().remove(0).dbModel();
            let details = ApiModel::mockedCountryDetails().remove(0);
            fixture.mockedWebRepo.actions.setExpected(vec![
                MockedCountriesWebRepositoryAction::details { country: country.clone() },
            ]);
            *fixture.mockedWebRepo.detailsResponses.borrow_mut() =
                vec![Ok(details.clone())];
            fixture.mockedDBRepo.actions.setExpected(vec![
                MockedCountriesDBRepositoryAction::storeDetails {
                    countryDetails: details.clone(),
                    country: country.clone(),
                },
                MockedCountriesDBRepositoryAction::fetchCountryDetails(country.clone()),
            ]);
            let expected = dbDetails(&country, &details);
            *fixture.mockedDBRepo.countryDetailsResults.borrow_mut() =
                vec![Ok(Some(expected.clone()))];
            *fixture.mockedDBRepo.storeCountryDetailsResults.borrow_mut() = vec![Ok(())];
            let result = block_on(fixture.sut.loadCountryDetails(country.clone(), true)).unwrap();
            assert_eq!(result, expected);
            fixture.mockedWebRepo.actions.verify();
            fixture.mockedDBRepo.actions.verify();
        }

        #[test]
        fn happyPathNoCache() {
            let fixture = CountriesInteractorTests::init();
            let country = ApiModel::mockedCountries().remove(0).dbModel();
            let details = ApiModel::mockedCountryDetails().remove(0);
            fixture.mockedWebRepo.actions.setExpected(vec![
                MockedCountriesWebRepositoryAction::details { country: country.clone() },
            ]);
            *fixture.mockedWebRepo.detailsResponses.borrow_mut() =
                vec![Ok(details.clone())];
            fixture.mockedDBRepo.actions.setExpected(vec![
                MockedCountriesDBRepositoryAction::fetchCountryDetails(country.clone()),
                MockedCountriesDBRepositoryAction::storeDetails {
                    countryDetails: details.clone(),
                    country: country.clone(),
                },
                MockedCountriesDBRepositoryAction::fetchCountryDetails(country.clone()),
            ]);
            let expected = dbDetails(&country, &details);
            *fixture.mockedDBRepo.countryDetailsResults.borrow_mut() =
                vec![Ok(None), Ok(Some(expected.clone()))];
            *fixture.mockedDBRepo.storeCountryDetailsResults.borrow_mut() = vec![Ok(())];
            let result = block_on(fixture.sut.loadCountryDetails(country.clone(), false)).unwrap();
            assert_eq!(result, expected);
            fixture.mockedWebRepo.actions.verify();
            fixture.mockedDBRepo.actions.verify();
        }

        #[test]
        fn cacheDBFailure() {
            let fixture = CountriesInteractorTests::init();
            let country = ApiModel::mockedCountries().remove(0).dbModel();
            let details = ApiModel::mockedCountryDetails().remove(0);
            fixture.mockedWebRepo.actions.setExpected(vec![
                MockedCountriesWebRepositoryAction::details { country: country.clone() },
            ]);
            *fixture.mockedWebRepo.detailsResponses.borrow_mut() =
                vec![Ok(details.clone())];
            fixture.mockedDBRepo.actions.setExpected(vec![
                MockedCountriesDBRepositoryAction::fetchCountryDetails(country.clone()),
                MockedCountriesDBRepositoryAction::storeDetails {
                    countryDetails: details.clone(),
                    country: country.clone(),
                },
                MockedCountriesDBRepositoryAction::fetchCountryDetails(country.clone()),
            ]);
            let expected = dbDetails(&country, &details);
            *fixture.mockedDBRepo.countryDetailsResults.borrow_mut() =
                vec![Err(NSError_test()), Ok(Some(expected.clone()))];
            *fixture.mockedDBRepo.storeCountryDetailsResults.borrow_mut() = vec![Ok(())];
            let result = block_on(fixture.sut.loadCountryDetails(country.clone(), false)).unwrap();
            assert_eq!(result, expected);
            fixture.mockedWebRepo.actions.verify();
            fixture.mockedDBRepo.actions.verify();
        }

        #[test]
        fn fetchAfterStoringDBFailure() {
            let fixture = CountriesInteractorTests::init();
            let country = ApiModel::mockedCountries().remove(0).dbModel();
            let details = ApiModel::mockedCountryDetails().remove(0);
            fixture.mockedWebRepo.actions.setExpected(vec![
                MockedCountriesWebRepositoryAction::details { country: country.clone() },
            ]);
            *fixture.mockedWebRepo.detailsResponses.borrow_mut() =
                vec![Ok(details.clone())];
            fixture.mockedDBRepo.actions.setExpected(vec![
                MockedCountriesDBRepositoryAction::fetchCountryDetails(country.clone()),
                MockedCountriesDBRepositoryAction::storeDetails {
                    countryDetails: details.clone(),
                    country: country.clone(),
                },
                MockedCountriesDBRepositoryAction::fetchCountryDetails(country.clone()),
            ]);
            let error = NSError_test();
            *fixture.mockedDBRepo.countryDetailsResults.borrow_mut() =
                vec![Ok(None), Err(error)];
            *fixture.mockedDBRepo.storeCountryDetailsResults.borrow_mut() = vec![Ok(())];
            // `await #expect(throws: ValueIsMissingError.self) { … }`
            assert_eq!(
                block_on(fixture.sut.loadCountryDetails(country.clone(), false)).unwrap_err(),
                ValueIsMissingError()
            );
            fixture.mockedWebRepo.actions.verify();
            fixture.mockedDBRepo.actions.verify();
        }

        #[test]
        fn storingDBFailure() {
            let fixture = CountriesInteractorTests::init();
            let country = ApiModel::mockedCountries().remove(0).dbModel();
            let details = ApiModel::mockedCountryDetails().remove(0);
            fixture.mockedWebRepo.actions.setExpected(vec![
                MockedCountriesWebRepositoryAction::details { country: country.clone() },
            ]);
            *fixture.mockedWebRepo.detailsResponses.borrow_mut() =
                vec![Ok(details.clone())];
            fixture.mockedDBRepo.actions.setExpected(vec![
                MockedCountriesDBRepositoryAction::fetchCountryDetails(country.clone()),
                MockedCountriesDBRepositoryAction::storeDetails {
                    countryDetails: details.clone(),
                    country: country.clone(),
                },
            ]);
            let error = NSError_test();
            *fixture.mockedDBRepo.countryDetailsResults.borrow_mut() = vec![Ok(None)];
            *fixture.mockedDBRepo.storeCountryDetailsResults.borrow_mut() =
                vec![Err(error.clone())];
            assert_eq!(
                block_on(fixture.sut.loadCountryDetails(country.clone(), false)).unwrap_err(),
                error
            );
            fixture.mockedWebRepo.actions.verify();
            fixture.mockedDBRepo.actions.verify();
        }

        #[test]
        fn webFailure() {
            let fixture = CountriesInteractorTests::init();
            let country = ApiModel::mockedCountries().remove(0).dbModel();
            fixture.mockedWebRepo.actions.setExpected(vec![
                MockedCountriesWebRepositoryAction::details { country: country.clone() },
            ]);
            let error = NSError_test();
            *fixture.mockedWebRepo.detailsResponses.borrow_mut() =
                vec![Err(error.clone())];
            fixture.mockedDBRepo.actions.setExpected(vec![
                MockedCountriesDBRepositoryAction::fetchCountryDetails(country.clone()),
            ]);
            *fixture.mockedDBRepo.countryDetailsResults.borrow_mut() = vec![Ok(None)];
            assert_eq!(
                block_on(fixture.sut.loadCountryDetails(country.clone(), false)).unwrap_err(),
                error
            );
            fixture.mockedWebRepo.actions.verify();
            fixture.mockedDBRepo.actions.verify();
        }
    }

    mod StubCountriesInteractorTests {
        use super::*;

        #[test]
        fn stubInteractor() {
            let country = ApiModel::mockedCountries().remove(0).dbModel();
            let sut = StubCountriesInteractor;
            block_on(sut.refreshCountriesList()).unwrap();
            assert_eq!(
                block_on(sut.loadCountryDetails(country, false)).unwrap_err(),
                ValueIsMissingError()
            );
        }
    }
}
