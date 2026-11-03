# PokéBonk Tauri

> [!WARNING]
> **Projet non officiel, fait pour le fun, sans aucun lien avec le créateur de PokéBonk.**
> Le jeu, les images (écran de chargement, icône, Poké Ball), les astuces et les textes viennent du projet original PokéBonk de Charcher.
> Le jeu : https://pokebonk.com · le créateur sur GitHub : https://github.com/CharcherGit · le client officiel : https://github.com/CharcherGit/pokebonk-app
> Si le créateur le demande, ce dépôt sera retiré.

Client de bureau non officiel pour [PokéBonk](https://pokebonk.com), écrit en Tauri 2 au lieu d'Electron. Il charge https://pokebonk.com dans le moteur web du système (WebKit sur macOS, WebView2 sur Windows) : le compte et la progression sont côté serveur, donc identiques à ceux du site.

Les images de `web/splash/`, `web/icon.png` et les textes de `src-tauri/src/texts.json` viennent du client officiel (© leur auteur).

## Télécharger

Les versions sont sur la page [Releases](https://github.com/Lucas-Rosenzweig/pokebonk-tauri/releases/latest).

**macOS (Apple Silicon et Intel)** : télécharger le `.dmg`, l'ouvrir, glisser l'app dans Applications. L'app n'est pas signée par un compte développeur Apple : au premier lancement macOS la bloque. Aller dans Réglages Système → Confidentialité et sécurité → « Ouvrir quand même », ou lancer dans le Terminal :

```bash
xattr -dr com.apple.quarantine "/Applications/PokéBonk Tauri.app"
```

**Windows 10/11** : télécharger `…_x64-setup.exe` et l'exécuter (installation pour l'utilisateur, sans droits administrateur). SmartScreen peut afficher « Windows a protégé votre ordinateur » : « Informations complémentaires » puis « Exécuter quand même ». La version Windows est encore peu testée.

## Pourquoi

Le client Electron 1.4.0 pour Mac plantait au lancement (les noms des Helpers contiennent un « é » encodé différemment, voir https://github.com/CharcherGit/pokebonk-app/issues/5). Ici : une app d'environ 12 Mo, sans processus auxiliaires.

## Fonctionnalités reprises du client Electron

- Pont `window.pokebonkDesktop` (même API, version de pont 1.4.0) : quit, display/setDisplay, fullscreen, relaunch, retry, presence, update.
- Écran de chargement par-dessus le jeu, qui suit le vrai chargement.
- Page hors ligne avec réessai automatique.
- Plein écran de la fenêtre (F11, ⌃⌘F) ; Échap reste au jeu.
- Confirmation avant de quitter ou recharger pendant une partie.
- Liens externes ouverts dans le navigateur.
- Une seule instance.
- Position, taille et plein écran mémorisés (plein écran au premier lancement).
- L'écran ne se met pas en veille pendant qu'on joue (manette).
- Discord Rich Presence (macOS/Linux).
- Pas de limitation en arrière-plan (co-op).
- Musique du menu sans clic (autoplay).
- Menu macOS avec copier/coller.

## Différences avec le client Electron

- Moteur WebKit au lieu de Chromium : les données locales du navigateur (localStorage) du client Electron ne sont pas reprises.
- Pas de choix de backend graphique ni de V-Sync (WebKit s'en charge).
- Pas de mise à jour automatique : le jeu propose le téléchargement, comme le client Electron sur Mac.
- Windows : l'installateur est construit par la CI mais l'app y est très peu testée. Discord n'y est pas encore disponible.
- Pas de relance automatique si le processus de la page plante.

## Prérequis

- Rust (stable, version 1.82 ou plus)
- Node et pnpm
- Xcode Command Line Tools

## Commandes

Installer les dépendances :

```bash
pnpm install
```

Lancer en développement :

```bash
pnpm dev
```

Construire l'app (produit `src-tauri/target/release/bundle/macos/PokéBonk Tauri.app`) :

```bash
pnpm build
```

L'installer en la copiant dans Applications :

```bash
cp -R "src-tauri/target/release/bundle/macos/PokéBonk Tauri.app" /Applications/
```

Régénérer les icônes à partir de `web/icon.png` :

```bash
pnpm icons
```

## Options de lancement

- `--url=<adresse>` ou variable `POKEBONK_URL` : charger une autre adresse, par exemple le jeu en local (`http://localhost:5173`). Cette origine reçoit aussi le pont.
- `--devtools` : activer les outils de développement.
- `--selftest=<dossier>` et `--selftest-wait=<ms>` (15000 par défaut) : lance l'app en fenêtre, attend, écrit `report.json` dans le dossier puis quitte, sans toucher aux réglages enregistrés. Le rapport contient : le délai de disparition de l'écran de chargement, les versions de l'app et du pont, l'URL finale, l'état plein écran et affichage, puis côté page : titre, user agent, langue, clés/version/plateforme du pont, état de l'interface (`loading`, `screen`…), si le jeu demanderait confirmation avant de partir, un appel IPC de test (`display()`), la taille d'écran et le moteur WebGL.
- `POKEBONK_DISCORD_ID` : identifiant d'application Discord à utiliser pour la Rich Presence.
- `POKEBONK_RPC_DEBUG` : afficher sur stderr les messages reçus de Discord.

## Structure

```
web/                      pages locales : splash.html, offline.html, images, polices
src-tauri/
  tauri.conf.json         configuration de l'app (identifiant, bundle .app)
  build.rs                déclare les commandes du pont (permissions)
  capabilities/game.json  ce que la page a le droit d'appeler
  src/
    lib.rs                fenêtre, webviews, menu, commandes du pont, sortie, auto-test
    splash.rs             écran de chargement
    display.rs            réglages d'affichage (pont display/setDisplay)
    discord.rs            Discord Rich Presence
    pointer_lock.rs       capture de la souris sur macOS : autorise le verrouillage du pointeur, que WKWebView refuse par défaut
    config.rs             client.json et window.json
    texts.rs, texts.json  textes traduits du client officiel
    bridge.js             script injecté qui expose window.pokebonkDesktop
```

## Publier une version

Pour le mainteneur : changer `version` dans `src-tauri/tauri.conf.json` (et dans `package.json` et `src-tauri/Cargo.toml`, pour les garder alignés), commiter, puis créer et pousser le tag :

```bash
git tag vX.Y.Z
```

```bash
git push origin vX.Y.Z
```

Le workflow Release construit le `.dmg` (universel) et l'installateur Windows, puis publie la release.

## Sécurité

La page ne peut appeler que les 9 commandes du pont, et seulement depuis pokebonk.com (et ses sous-domaines) ou les pages locales (`capabilities/game.json`). Rien ne touche aux fichiers ni au shell.

## Données

La configuration est dans `~/Library/Application Support/io.github.lucas-rosenzweig.pokebonk-tauri/` (`client.json`, `window.json`).
