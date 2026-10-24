tidalfast_overlay.so: tidalfast_overlay.c
	$(CC) -O2 -Wall -Wextra -fPIC -shared -o $@ $< -lpthread -ldl

clean:
	rm -f tidalfast_overlay.so
