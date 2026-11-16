#!/usr/bin/env bash
# Publication d'une version, de bout en bout (voir RELEASING.md).
#
#   scripts/release.sh check            vérifie que le dépôt est prêt à être étiqueté
#   scripts/release.sh tag              vérifie, pose l'étiquette vX.Y.Z et la pousse
#                                       (la CD construit alors une release en brouillon)
#   scripts/release.sh verify           télécharge le brouillon et le contrôle
#   scripts/release.sh publish          contrôle le brouillon puis le publie
#
# La version est celle du Cargo.toml. Rien n'est publié sans `publish`.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"
fail() { echo "erreur : $*" >&2; exit 1; }
step() { echo "→ $*"; }

[[ $# == 1 ]] || { sed -n '2,11p' "$0" >&2; exit 2; }
command -v gh >/dev/null || fail "gh (GitHub CLI) est requis"
VERSION=$(scripts/check-version.sh)
TAG="v$VERSION"
REPO=${RELEASE_REPO:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}

check() {
    step "version $VERSION cohérente (Cargo, RPM, metainfo, CHANGELOG)"
    scripts/check-version.sh "$TAG" >/dev/null

    step "arbre de travail propre, sur main, identique à origin/main"
    [[ -z $(git status --porcelain) ]] || fail "des modifications ne sont pas validées"
    [[ $(git rev-parse --abbrev-ref HEAD) == main ]] || fail "il faut être sur la branche main"
    git fetch --quiet origin main --tags
    [[ $(git rev-parse HEAD) == $(git rev-parse origin/main) ]] || fail "main diffère de origin/main (pousser ou tirer d'abord)"

    step "étiquette $TAG encore libre"
    ! git rev-parse -q --verify "refs/tags/$TAG" >/dev/null || fail "l'étiquette $TAG existe déjà"
    [[ -z $(git ls-remote --tags origin "refs/tags/$TAG") ]] || fail "l'étiquette $TAG existe déjà sur origin"

    step "identité de l'étiquette"
    local email
    email=$(git config user.email || true)
    [[ -n $email ]] || fail "git config user.email n'est pas réglé"
    # L'adresse apparaît dans l'étiquette publique : pas d'adresse personnelle par mégarde.
    if [[ -n ${RELEASE_EMAIL_DOMAIN:-wizops.fr} && $email != *"@${RELEASE_EMAIL_DOMAIN:-wizops.fr}" ]]; then
        fail "adresse « $email » hors du domaine ${RELEASE_EMAIL_DOMAIN:-wizops.fr} (régler git config user.email, ou RELEASE_EMAIL_DOMAIN= pour passer outre)"
    fi

    step "CI verte sur ce commit"
    local sha conclusion
    sha=$(git rev-parse HEAD)
    conclusion=$(gh run list --repo "$REPO" --workflow CI --commit "$sha" --limit 1 --json conclusion --jq '.[0].conclusion // "absente"')
    [[ $conclusion == success ]] || fail "CI du commit ${sha:0:7} : $conclusion (attendre qu'elle soit verte)"
    echo "prêt : $TAG peut être posée sur ${sha:0:7}"
}

verify() {
    step "release $TAG sur $REPO"
    local state
    state=$(gh release view "$TAG" --repo "$REPO" --json isDraft --jq 'if .isDraft then "brouillon" else "publiée" end' 2>/dev/null) \
        || fail "aucune release $TAG (la CD a-t-elle fini ? gh run list --repo $REPO --workflow Release)"
    echo "  état : $state"

    local dir
    dir=$(mktemp -d)
    # shellcheck disable=SC2064  # le chemin est figé maintenant, à dessein
    trap "rm -rf '$dir'" EXIT
    step "téléchargement des fichiers"
    gh release download "$TAG" --repo "$REPO" --dir "$dir"
    local archive="lightning-lcd-$VERSION-x86_64-linux.tar.gz"
    for f in "$archive" lightning lightning-settings SHA256SUMS; do
        [[ -s $dir/$f ]] || fail "fichier manquant dans la release : $f"
    done

    step "sommes SHA-256"
    (cd "$dir" && sha256sum --quiet -c SHA256SUMS)

    step "attestations de provenance (construits par la CD de $REPO)"
    for f in "$archive" lightning lightning-settings; do
        gh attestation verify "$dir/$f" --repo "$REPO" >/dev/null || fail "attestation invalide pour $f"
    done

    step "contenu de l'archive et version des binaires"
    tar -xzf "$dir/$archive" -C "$dir"
    local top="$dir/lightning-lcd-$VERSION-x86_64-linux"
    for f in bin/lightning bin/lightning-settings install.sh LICENSE README.md \
        packaging/70-lightning-lcd.rules packaging/lightning-dashboard.service; do
        [[ -e $top/$f ]] || fail "absent de l'archive : $f"
    done
    cmp -s "$top/bin/lightning" "$dir/lightning" || fail "le binaire de l'archive diffère du binaire seul"
    chmod +x "$top/bin/lightning" "$top/bin/lightning-settings"
    [[ $("$top/bin/lightning" --version) == "lightning $VERSION" ]] || fail "lightning --version ne dit pas $VERSION"
    [[ $("$top/bin/lightning-settings" --version) == "lightning-settings $VERSION" ]] || fail "lightning-settings --version ne dit pas $VERSION"

    step "notes de version"
    gh release view "$TAG" --repo "$REPO" --json body --jq .body | grep -q . || fail "notes de version vides"
    echo "release $TAG vérifiée"
}

case $1 in
    check) check ;;
    tag)
        check
        step "étiquette annotée $TAG"
        git tag -a "$TAG" -m "lightning-lcd $VERSION"
        git push --quiet origin "refs/tags/$TAG"
        echo "étiquette poussée. Suivre la construction : gh run watch --repo $REPO \$(gh run list --repo $REPO --workflow Release --limit 1 --json databaseId --jq '.[0].databaseId')"
        echo "puis : scripts/release.sh verify && scripts/release.sh publish"
        ;;
    verify) verify ;;
    publish)
        verify
        step "publication"
        gh release edit "$TAG" --repo "$REPO" --draft=false --latest >/dev/null
        gh release view "$TAG" --repo "$REPO" --json url,isDraft --jq '"publiée : \(.url) (brouillon : \(.isDraft))"'
        ;;
    *) sed -n '2,11p' "$0" >&2; exit 2 ;;
esac
