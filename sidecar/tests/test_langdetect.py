from __future__ import annotations

from llmtranslator_sidecar.langdetect import guess_language

FRENCH = (
    "A la demande d'Edwin, Duom Nil' Erg fit contourner au chariot le lieu de l'affrontement. "
    "Salim se leva pour voir les corps des pillards, mais Edwin le rabroua. Il était un des rares "
    "adultes capables de l'impressionner, et elle ne savait pas pourquoi. "
) * 4
ENGLISH = (
    "At Edwin's request, the carriage went around the site of the fight. Salim stood up to see "
    "the bodies of the raiders, but Edwin rebuked him. He was one of the few adults that could "
    "impress his friend, and it was not the first time. "
) * 4
ITALIAN = (
    "Su richiesta di Edwin, il carro aggirò il luogo dello scontro. Salim si alzò per vedere i "
    "corpi dei predoni, ma Edwin lo rimproverò. Era uno dei pochi adulti che non si lasciava "
    "impressionare, e per questo la ragazza gli era grata. "
) * 4


def test_guesses_the_language_of_prose() -> None:
    assert guess_language(FRENCH) == "fr"
    assert guess_language(ENGLISH) == "en"
    assert guess_language(ITALIAN) == "it"


def test_says_nothing_about_too_little_text() -> None:
    assert guess_language("Chapitre 21") is None
    assert guess_language("") is None
