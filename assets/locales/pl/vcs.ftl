## FerroIDE — Kontrola wersji: Commit, historia Git, porównywanie i scalanie.
## Każdy komunikat musi istnieć także w en/vcs.ftl (sprawdzane testem).

## Wspólne
vcs-refresh = Odśwież
vcs-retry = Spróbuj ponownie
vcs-show-diff = Pokaż różnice
vcs-file-count = { $count ->
    [one] { $count } plik
    [few] { $count } pliki
   *[other] { $count } plików
}
vcs-time-now = przed chwilą
vcs-time-minutes = { $n } min temu
vcs-time-hours = { $n } godz. temu
vcs-time-days = { $n ->
    [one] { $n } dzień temu
   *[other] { $n } dni temu
}
vcs-kind-modified = zmieniony
vcs-kind-added = dodany
vcs-kind-deleted = usunięty
vcs-kind-renamed = przemianowany
vcs-kind-untracked = niewersjonowany
vcs-kind-conflicted = konflikt scalania

## Okno Commit
vcs-changes = Zmiany
vcs-loading-changes = Odczytywanie zmian…
vcs-changes-failed = Nie udało się odczytać zmian
vcs-no-changes = Brak zmian
vcs-rollback = Wycofaj
vcs-rollback-not-available = Wycofywanie nie jest dostępne na danych testowych.
vcs-expand-all = Rozwiń wszystko
vcs-collapse-all = Zwiń wszystko
vcs-amend = Popraw ostatni commit
vcs-amend-not-available = Poprawianie commitu nie jest dostępne na danych testowych; powstanie nowy commit.
vcs-selected-count = { $count ->
    [one] wybrano { $count } plik
    [few] wybrano { $count } pliki
   *[other] wybrano { $count } plików
}
vcs-commit-message = Opis commitu
vcs-commit-message-placeholder = Opis commitu
vcs-commit = Zatwierdź
vcs-committed = Zatwierdzono { $count ->
    [one] { $count } plik
    [few] { $count } pliki
   *[other] { $count } plików
} jako { $hash }

## Historia Git
vcs-log = Historia Git
vcs-log-filter-placeholder = Filtruj po tekście, autorze lub skrócie
vcs-log-branch-filter = Filtruj po gałęzi
vcs-log-all-branches = Wszystkie gałęzie
vcs-log-loading = Odczytywanie historii…
vcs-log-failed = Nie udało się odczytać historii
vcs-log-empty = Brak commitów
vcs-log-no-match = Żaden commit nie pasuje do filtra
vcs-log-details = Szczegóły commitu
vcs-log-authored = { $author }, { $date }
vcs-log-merge-of = Scalenie { $count } rodziców

## Porównywanie
vcs-diff = Różnice
vcs-diff-tab = { $file } (różnice)
vcs-diff-loading = Obliczanie różnic…
vcs-diff-failed = Nie udało się obliczyć różnic
vcs-diff-no-changes = Plik nie ma zmian
vcs-diff-side-by-side = Obok siebie
vcs-diff-unified = Ujednolicony
vcs-diff-next-change = Następna zmiana (F7)
vcs-diff-previous-change = Poprzednia zmiana (Shift+F7)

## Scalanie
vcs-merge = Scalanie
vcs-merge-tab = { $file } (scalanie)
vcs-merge-loading = Odczytywanie konfliktów…
vcs-merge-failed = Nie udało się odczytać konfliktów
vcs-merge-no-conflicts = Brak konfliktów scalania.
vcs-merge-no-conflicts-in-file = Ten plik nie ma konfliktów scalania
vcs-merge-ours = Twoje ({ $label })
vcs-merge-theirs = Ich ({ $label })
vcs-merge-base = Wersja bazowa
vcs-merge-result = Wynik
vcs-merge-unresolved = Nierozwiązany konflikt
vcs-merge-accept-left = Przyjmij lewe
vcs-merge-accept-right = Przyjmij prawe
vcs-merge-accept-both = Przyjmij oba
vcs-merge-ignore = Pomiń
vcs-merge-next-conflict = Następny konflikt (F7)
vcs-merge-previous-conflict = Poprzedni konflikt (Shift+F7)
vcs-merge-counter = Konflikt { $current } z { $total } · nierozwiązane: { $unresolved }
vcs-merge-show-base = Pokaż bazę
vcs-merge-apply = Zastosuj
vcs-merge-done = Rozwiązano
vcs-merge-applied = { $file }: konflikty rozwiązane
