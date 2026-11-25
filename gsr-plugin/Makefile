oxytide_overlay.so: oxytide_overlay.c
	$(CC) -O2 -Wall -Wextra -fPIC -shared -o $@ $< -lpthread -ldl

clean:
	rm -f oxytide_overlay.so
