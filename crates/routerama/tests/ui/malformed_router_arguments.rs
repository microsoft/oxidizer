// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::router;

mod unknown {
    use super::router;

    struct AppState;
    struct Api;

    #[router(context = AppState)]
    impl Api {
        #[route(GET, "/")]
        async fn home(&self) -> &'static str {
            "home"
        }
    }
}

mod duplicate {
    use super::router;

    struct AppState;
    struct OtherState;
    struct Api;

    #[router(state = AppState, state = OtherState)]
    impl Api {
        #[route(GET, "/")]
        async fn home(&self) -> &'static str {
            "home"
        }
    }
}

mod trailing {
    use super::router;

    struct AppState;
    struct Api;

    #[router(state = AppState, unexpected)]
    impl Api {
        #[route(GET, "/")]
        async fn home(&self) -> &'static str {
            "home"
        }
    }
}

mod anonymous_lifetime {
    use super::router;

    struct AppState<'a>(&'a str);
    struct Api;

    #[router(state = AppState<'_>)]
    impl Api {
        #[route(GET, "/")]
        async fn home(&self) -> &'static str {
            "home"
        }
    }
}

mod omitted_lifetime {
    use super::router;

    struct AppState<'a>(&'a str);
    struct Api;

    #[router(state = AppState)]
    impl Api {
        #[route(GET, "/")]
        async fn home(&self) -> &'static str {
            "home"
        }
    }
}

mod erased_mounts_without_state {
    use super::router;

    struct Api;

    #[router(erased_mounts)]
    impl Api {
        #[route(GET, "/")]
        async fn home(&self) -> &'static str {
            "home"
        }
    }
}

mod duplicate_erased_mounts {
    use super::router;

    struct AppState;
    struct Api;

    #[router(state = AppState, erased_mounts, erased_mounts)]
    impl Api {
        #[route(GET, "/")]
        async fn home(&self) -> &'static str {
            "home"
        }
    }
}

fn main() {}
