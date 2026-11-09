## FerroIDE — Okna dialogowe: zmiana nazwy, nowy plik, konfiguracje uruchomieniowe, klonowanie repozytorium.
## Każdy komunikat musi istnieć także w en/dialogs.ftl (sprawdzane testem).

## Nowy plik
dialogs-new-file-name = Nazwa
dialogs-new-file-name-placeholder = np. utils.rs
dialogs-new-file-directory = Katalog (względem projektu)
dialogs-new-file-directory-placeholder = katalog główny projektu
dialogs-new-file-kind-file = Plik
dialogs-new-file-kind-directory = Katalog
dialogs-new-file-create = Utwórz
dialogs-new-file-error-empty = Podaj nazwę.
dialogs-new-file-error-invalid = Nazwa zawiera niedozwolone znaki albo zaczyna się lub kończy spacją lub kropką.
dialogs-new-file-error-outside = Katalog musi leżeć wewnątrz projektu.

## Konfiguracje uruchomieniowe
dialogs-run-title = Konfiguracje uruchomieniowe
dialogs-run-name = Nazwa
dialogs-run-kind = Typ
dialogs-run-command = Polecenie
dialogs-run-arguments = Argumenty programu
dialogs-run-working-directory = Katalog roboczy
dialogs-run-environment = Zmienne środowiskowe
dialogs-run-env-add = Dodaj zmienną
dialogs-run-env-value = wartość
dialogs-run-add = Dodaj konfigurację
dialogs-run-duplicate = Duplikuj
dialogs-run-remove = Usuń
dialogs-run-empty = Brak konfiguracji uruchomieniowych
dialogs-run-unnamed = Bez nazwy
dialogs-run-copy = kopia
dialogs-run-error-name = Każda konfiguracja musi mieć nazwę.
dialogs-run-error-command = Każda konfiguracja musi mieć polecenie.

## Klonowanie repozytorium
dialogs-clone-title = Klonuj repozytorium
dialogs-clone-url = Adres repozytorium
dialogs-clone-directory = Katalog
dialogs-clone-button = Klonuj
dialogs-clone-stop = Zatrzymaj
dialogs-clone-progress = Klonowanie…
dialogs-clone-error-url-empty = Podaj adres repozytorium.
dialogs-clone-error-url-invalid = To nie wygląda na adres Git (https://…, ssh://…, git@host:ścieżka lub ścieżka kończąca się na .git).
dialogs-clone-error-directory = Wybierz katalog docelowy.

## Zmiana nazwy
dialogs-rename-symbol = Zmień nazwę „{ $name }” i jego użyć na:
dialogs-rename-new-name = Nowa nazwa
dialogs-rename-apply = Zmień nazwę
dialogs-rename-type-name = Wpisz nową nazwę, aby zobaczyć zmiany.
dialogs-rename-loading = Wyszukiwanie użyć…
dialogs-rename-no-usages = Nie znaleziono użyć.
dialogs-rename-summary = { $count ->
    [one] 1 zmiana
    [few] { $count } zmiany
   *[other] { $count } zmian
} w { $files ->
    [one] 1 pliku
   *[other] { $files } plikach
}
dialogs-rename-done = Zmieniono nazwę w { $count ->
    [one] 1 miejscu
   *[other] { $count } miejscach
} w { $files ->
    [one] 1 pliku
   *[other] { $files } plikach
}.
dialogs-rename-skipped = { $files ->
    [one] 1 plik nie jest otwarty i pozostał bez zmian.
    [few] { $files } pliki nie są otwarte i pozostały bez zmian.
   *[other] { $files } plików nie jest otwartych i pozostało bez zmian.
}
dialogs-rename-no-symbol = Umieść kursor na symbolu, aby zmienić jego nazwę.
dialogs-rename-error-empty = Podaj nazwę.
dialogs-rename-error-invalid = To nie jest poprawny identyfikator.
dialogs-rename-error-keyword = Słowo kluczowe nie może być nazwą.
