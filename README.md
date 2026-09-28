# omarchy-armoury

ASUS ROG control for Omarchy: a Rust daemon (`armouryd`), CLI (`armoury`),
root helper (`armoury-root`) and, in later milestones, an Omarchy bar popup
and window. Replaces G-Helper. Design: `docs/superpowers/specs/`.

## Install

    ./install.sh

armouryd starts in **observe** mode: it reads hardware state and changes
nothing, so G-Helper keeps working.

    armoury status          # what the daemon sees
    armoury takeover        # stop G-Helper, start asusd, armouryd controls hardware
    armoury handback        # stop asusd, restart G-Helper

While active, G-Helper will not autostart at login.

## asusd lighting fix

asusd's model database lists the G533Z as keyboard-only, so the logo and
lightbar never light. `armoury-root asusd-support-fix` adds the missing zones;
a pacman hook re-applies it after asusctl upgrades.

## Uninstall

    ./uninstall.sh
