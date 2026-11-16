# Politique de sécurité

## Versions prises en charge

Seule la dernière version publiée (et la branche `main`) reçoit des correctifs de sécurité.

## Signaler une vulnérabilité

**Ne publiez pas de vulnérabilité dans une issue publique.** Utilisez de préférence le
signalement privé de GitHub (onglet *Security* → *Report a vulnerability*) ou écrivez à
**contact@wizops.fr** avec :

- la version (`lightning --version`, `lightning-settings --version`) et la distribution ;
- la description du problème et son impact ;
- les étapes pour le reproduire, idéalement sans le matériel (`--no-lcd --demo`).

Nous accusons réception sous 7 jours et visons un correctif sous 30 jours selon la gravité. Le
signalement est crédité dans les notes de version, sauf si vous préférez rester anonyme.

## Périmètre

Sont notamment concernés :

- le démon `lightning daemon` et son interface D-Bus `cloud.wizops.Lightning1` (bus de session :
  tout processus de l'utilisateur peut le piloter, c'est voulu ; une élévation au-delà de cet
  utilisateur ne l'est pas) ;
- l'aperçu HTTP (`--output http`), en lecture seule et **sans authentification** : il écoute sur
  `127.0.0.1` par défaut ; l'ouvrir sur le réseau est un choix explicite de l'utilisateur ;
- la règle udev (`TAG+="uaccess"` limité à `0db0:7881`) et l'unité systemd utilisateur ;
- le stockage du jeton Google Photos (`~/.config/lightning/google.json`, mode 0600) et le
  téléchargement des photos choisies ;
- le décodage des images, vidéos (via `ffmpeg`) et fichiers de configuration.

Hors périmètre : les défauts du firmware de l'écran lui-même (à signaler au constructeur), et les
attaques qui supposent déjà un accès physique ou le compte de l'utilisateur.
